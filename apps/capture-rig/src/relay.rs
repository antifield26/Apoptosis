//! The relay: byte-exact forwarding with observation on the side.

use crate::{Direction, Session};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Read buffer per direction. A frame can be up to 2 MiB, but TCP hands over what it has and the codec
/// reassembles, so this is only a chunk size.
const CHUNK: usize = 16 * 1024;

/// How long to keep draining the surviving direction after the other one ends.
///
/// Long enough for a final chunk in flight, short enough that a peer which never closes cannot hold a
/// capture open — an unfinished trace is indistinguishable from a running one, which would make every count
/// in it unverifiable.
const SESSION_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Relay one connection until both directions close.
///
/// Both directions are pumped concurrently and this returns when both have finished: a client disconnect
/// closes the upstream write half, the server then closes, and the other pump ends. Nothing here interprets
/// the bytes — [`Session::observe`] is a side channel whose failure cannot affect forwarding.
///
/// # Errors
///
/// Propagates I/O errors from either direction. A capture ending because the client went away is not an
/// error at this level; the caller decides.
pub async fn relay_pair<C, U>(
    client: C,
    upstream: U,
    session: Arc<Mutex<Session>>,
) -> std::io::Result<()>
where
    C: AsyncRead + AsyncWrite + Unpin,
    U: AsyncRead + AsyncWrite + Unpin,
{
    let (client_read, client_write) = tokio::io::split(client);
    let (upstream_read, upstream_write) = tokio::io::split(upstream);

    let to_server = pump(
        client_read,
        upstream_write,
        Arc::clone(&session),
        Direction::ClientToServer,
    );
    let to_client = pump(
        upstream_read,
        client_write,
        session,
        Direction::ServerToClient,
    );
    tokio::pin!(to_server);
    tokio::pin!(to_client);

    // Finish as soon as **either** direction ends, then drain the other with a bound.
    //
    // `join!` was wrong here: a server that keeps its half open after the client leaves would leave the
    // relay waiting forever, so `session_end` was never written and the capture had no end marker. Found by
    // the join integration test; the status test passed only because that server closes by itself.
    tokio::select! {
        result = &mut to_server => {
            result?;
            let _ = tokio::time::timeout(SESSION_DRAIN_GRACE, &mut to_client).await;
        }
        result = &mut to_client => {
            result?;
            let _ = tokio::time::timeout(SESSION_DRAIN_GRACE, &mut to_server).await;
        }
    }
    Ok(())
}

/// Forward one direction, observing as it goes.
async fn pump<R, W>(
    mut read: R,
    mut write: W,
    session: Arc<Mutex<Session>>,
    direction: Direction,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read_bytes = read.read(&mut buffer).await?;
        if read_bytes == 0 {
            break;
        }
        let chunk = &buffer[..read_bytes];
        // Forward first, and unconditionally. Observation is a side channel; if the lock were poisoned the
        // trace would be lost, but the session under observation would not be.
        write.write_all(chunk).await?;
        write.flush().await?;
        match session.lock() {
            Ok(mut guard) => guard.observe(direction, chunk),
            Err(poisoned) => poisoned.into_inner().observe(direction, chunk),
        }
    }
    Ok(())
}

/// Accept connections and relay each one, writing a trace per connection.
///
/// `new_sink` is called once per connection, so a caller decides where traces go: a file opened in append
/// mode, one file per session, or an in-memory buffer for a test. Passing a factory rather than a sink is
/// what keeps `serve` from owning a policy it has no business owning.
///
/// `max_connections` bounds how many are served before returning, which is what makes a scripted session
/// scriptable: the caller knows when the rig is finished.
///
/// # Errors
///
/// Propagates accept errors. A failed relay for one connection is logged and does **not** stop the
/// listener: a capture rig that dies on the first surprise cannot capture the surprise.
pub async fn serve<F>(
    listener: tokio::net::TcpListener,
    upstream: std::net::SocketAddr,
    max_connections: Option<usize>,
    body_dir: Option<std::path::PathBuf>,
    mut new_sink: F,
) -> std::io::Result<()>
where
    F: FnMut() -> Box<dyn Write + Send>,
{
    let mut served = 0usize;
    loop {
        let (client, peer) = listener.accept().await?;
        let sink = new_sink();
        let mut session = Session::new(sink);
        // Opt-in and threaded rather than global: the rig owns no policy about where traces go, which is
        // why the sink is already a caller-supplied factory.
        if let Some(dir) = &body_dir {
            session.dump_bodies_to(dir.clone());
        }
        let session = Arc::new(Mutex::new(session));
        let started = Instant::now();
        if let Err(error) = relay_one(client, upstream, Arc::clone(&session), peer).await {
            tracing::warn!(%error, %peer, "relay ended with an error");
        }
        // Saturating, not truncating: a session longer than 584 million years is not a
        // case worth panicking over, and the cast lint is right that this can lose data.
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match session.lock() {
            Ok(mut guard) => {
                guard.end(elapsed);
                let _ = guard.flush();
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.end(elapsed);
                let _ = guard.flush();
            }
        }
        served += 1;
        if max_connections.is_some_and(|max| served >= max) {
            return Ok(());
        }
    }
}

/// Relay a single accepted connection.
async fn relay_one(
    client: TcpStream,
    upstream: std::net::SocketAddr,
    session: Arc<Mutex<Session>>,
    peer: std::net::SocketAddr,
) -> std::io::Result<()> {
    let upstream_stream = TcpStream::connect(upstream).await?;
    match session.lock() {
        Ok(mut guard) => guard.start(&peer.to_string(), &upstream.to_string()),
        Err(poisoned) => poisoned
            .into_inner()
            .start(&peer.to_string(), &upstream.to_string()),
    }
    relay_pair(client, upstream_stream, session).await
}
