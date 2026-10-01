//! RCON TCP listener (P19-04).
//!
//! Vanilla's admin protocol on its own port: length-prefixed little-endian
//! packets, password login, then console-level commands with chunked
//! replies. The listener owns sockets, auth and throttle; the game owns
//! meaning — every command crosses into the tick loop as a
//! [`mc_server::rcon::RconRequest`] and the joined reply lines come back
//! over a one-shot.
//!
//! Hostile-input posture (AUDIT-19; pinned by the tests in this module, which
//! drive real loopback sockets as well as in-memory ones):
//!
//! - a length prefix above [`mc_server::rcon::MAX_PACKET_LEN`] (or below
//!   the 10-byte header) closes the connection **before** any payload byte
//!   is allocated or read;
//! - a truncated body (EOF mid-packet) closes it;
//! - a body that arrives in several reads is reassembled (`read_exact`), where
//!   Vanilla closes on anything that is not the exact size of one 1460-byte
//!   read — our bound is [`mc_server::rcon::MAX_PACKET_LEN`] and it is ours,
//!   not the jar's (AUDIT-19 C19-L1; the divergence is spelled out at that
//!   constant's documentation);
//! - an unknown packet type closes it;
//! - a wrong password answers id `-1`, sleeps the backoff, and after
//!   [`MAX_AUTH_FAILURES`] consecutive failures the connection is closed;
//! - a socket that has not logged in within [`PREAUTH_TIMEOUT`] is closed, so
//!   an idle pre-auth socket is not free (AUDIT-19 A-05/C19-M1);
//! - concurrent sockets are admitted through the game listener's
//!   [`mc_server::rcon::listener_gate`] — the surplus is refused at `accept`
//!   rather than becoming another task (AUDIT-19 A-05);
//! - bad logins accumulate per source address in
//!   [`mc_server::rcon::AuthBudget`], so reconnecting does not reset the
//!   throttle (AUDIT-19 C19-M2);
//! - a failed `accept` is logged and backed off instead of spinning hot with
//!   nothing in the log (AUDIT-19 C19-L4);
//! - per connection everything is sequential, so a flooding client only
//!   queues behind itself.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mc_server::rcon::{
    AUTH_FAILURE_ID, AuthBudget, ConnectionGate, RconRequest, TYPE_COMMAND, TYPE_LOGIN, auth_delay,
    check_password, decode_body, encode_packet, encode_response, length_accepted, listener_gate,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Consecutive bad logins before the connection is closed.
pub const MAX_AUTH_FAILURES: u32 = 5;

/// How long a pre-authentication socket may stay silent (AUDIT-19 A-05).
///
/// A stock client writes its login packet immediately after `connect`; ten
/// seconds leaves room for a human-driven tool or a slow link while bounding
/// what an idle socket costs. The clock is per read, so a client that dribbles
/// a header one byte at a time is bounded by the connection budget instead.
pub const PREAUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Pause after a failed `accept` before trying again (AUDIT-19 C19-L4).
///
/// The game listener only warns; RCON also waits, because a persistent accept
/// error (a closed descriptor, `EMFILE`) would otherwise spin at full speed
/// with nothing to show for it but the warning itself.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Everything the listener needs to bound a hostile client.
#[derive(Debug)]
pub struct RconLimits {
    /// Concurrent-socket budget, shared with the game listener's limiter.
    pub gate: Arc<ConnectionGate>,
    /// Bad-login budget, shared per address across reconnecting clients.
    pub auth: Arc<AuthBudget>,
    /// How long a socket may stay silent before it logs in.
    pub preauth_timeout: Duration,
}

impl Default for RconLimits {
    fn default() -> Self {
        Self {
            gate: listener_gate(),
            auth: Arc::new(AuthBudget::default()),
            preauth_timeout: PREAUTH_TIMEOUT,
        }
    }
}

/// Serve RCON until the task is aborted (the binary aborts it after
/// `Server::run` returns).
pub async fn serve(
    bind: std::net::SocketAddr,
    password: Vec<u8>,
    commands: tokio::sync::mpsc::Sender<RconRequest>,
) -> mc_core::error::ServerResult<()> {
    if !bind.ip().is_loopback() {
        tracing::warn!(
            %bind,
            "RCON is bound off loopback: its password crosses the network in the clear"
        );
    }
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| mc_core::error::ServerError::Operational(format!("cannot bind RCON: {e}")))?;
    tracing::info!(%bind, "RCON listening");
    serve_listener(
        listener,
        password,
        commands,
        Arc::new(RconLimits::default()),
    )
    .await
}

/// Accept loop over an already-bound listener (`serve` binds; tests inject).
///
/// Every socket passes three doors before it becomes a task: the address
/// budget (bad logins), the connection gate (concurrent sockets and reconnect
/// rate), and only then `tokio::spawn`. A refused socket is dropped, so the
/// listener can never hold more tasks than the gate admits.
pub async fn serve_listener(
    listener: TcpListener,
    password: Vec<u8>,
    commands: tokio::sync::mpsc::Sender<RconRequest>,
    limits: Arc<RconLimits>,
) -> mc_core::error::ServerResult<()> {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // A persistent accept error must be visible, and must not spin
                // at full speed while it lasts (AUDIT-19 C19-L4).
                tracing::warn!(%error, "RCON accept failed");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                continue;
            }
        };
        let now = Instant::now();
        if limits.auth.is_blocked(peer.ip(), now) {
            tracing::warn!(
                %peer,
                "RCON connection refused: too many bad logins from this address"
            );
            drop(stream);
            continue;
        }
        let guard = match Arc::clone(&limits.gate).try_acquire(peer.ip(), now) {
            Ok(guard) => guard,
            Err(reason) => {
                tracing::warn!(%peer, ?reason, "RCON connection refused by the connection budget");
                drop(stream);
                continue;
            }
        };
        tracing::debug!(%peer, "RCON connection");
        let password = password.clone();
        let commands = commands.clone();
        let limits = Arc::clone(&limits);
        tokio::spawn(async move {
            // The guard is this socket's admission slot: held for the whole
            // task, released on drop when the connection ends.
            let _guard = guard;
            if let Err(error) = serve_connection(stream, peer, &password, &commands, &limits).await
            {
                tracing::debug!(%peer, %error, "RCON connection closed");
            }
        });
    }
}

/// Read one framed body: the length prefix, capped before any allocation.
async fn read_body(
    reader: &mut (impl AsyncReadExt + Unpin),
) -> mc_core::error::ServerResult<Vec<u8>> {
    let length = reader.read_i32_le().await.map_err(|_| {
        mc_core::error::ServerError::Protocol("rcon EOF in the length prefix".to_owned())
    })?;
    let Ok(length) = usize::try_from(length) else {
        return Err(mc_core::error::ServerError::Protocol(format!(
            "rcon declared a negative length ({length})"
        )));
    };
    if !length_accepted(length) {
        return Err(mc_core::error::ServerError::Protocol(format!(
            "rcon declared {length} bytes, above the {cap}-byte cap",
            cap = mc_server::rcon::MAX_PACKET_LEN
        )));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.map_err(|_| {
        mc_core::error::ServerError::Protocol("rcon EOF inside the body".to_owned())
    })?;
    Ok(body)
}

async fn write_all(
    writer: &mut (impl AsyncWriteExt + Unpin),
    bytes: &[u8],
) -> mc_core::error::ServerResult<()> {
    writer
        .write_all(bytes)
        .await
        .map_err(|e| mc_core::error::ServerError::Operational(format!("rcon write failed: {e}")))
}

/// One connection: logins until one succeeds (or a budget runs out), then
/// commands while auth holds.
///
/// Two budgets can end the login phase: this connection's own
/// [`MAX_AUTH_FAILURES`], and the address-wide
/// [`mc_server::rcon::AuthBudget`] the listener shares between connections, so
/// a client cannot reconnect its way back to a clean counter.
async fn serve_connection(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    peer: SocketAddr,
    password: &[u8],
    commands: &tokio::sync::mpsc::Sender<RconRequest>,
    limits: &RconLimits,
) -> mc_core::error::ServerResult<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut failures = 0u32;
    loop {
        // A pre-auth socket is not an open door: silence has a deadline
        // (AUDIT-19 A-05/C19-M1).
        let body = match tokio::time::timeout(limits.preauth_timeout, read_body(&mut reader)).await
        {
            Ok(body) => body?,
            Err(_) => {
                return Err(mc_core::error::ServerError::Protocol(format!(
                    "rcon login did not arrive within {:?}",
                    limits.preauth_timeout
                )));
            }
        };
        let login = decode_body(&body)?;
        if login.kind != TYPE_LOGIN {
            return Err(mc_core::error::ServerError::Protocol(
                "rcon spoke before logging in".to_owned(),
            ));
        }
        if check_password(&login.payload, password) {
            // A real operator got in; the address's failure run is over.
            limits.auth.clear(peer.ip());
            write_all(&mut writer, &encode_packet(login.id, TYPE_COMMAND, &[])).await?;
            break;
        }
        write_all(
            &mut writer,
            &encode_packet(AUTH_FAILURE_ID, TYPE_COMMAND, &[]),
        )
        .await?;
        failures += 1;
        let total = limits.auth.record_failure(peer.ip(), Instant::now());
        tracing::warn!(%peer, failures, total, "RCON login failed");
        if failures >= MAX_AUTH_FAILURES {
            return Err(mc_core::error::ServerError::Protocol(format!(
                "rcon closed after {failures} bad logins"
            )));
        }
        if limits.auth.is_blocked(peer.ip(), Instant::now()) {
            return Err(mc_core::error::ServerError::Protocol(format!(
                "rcon closed after {total} bad logins from this address"
            )));
        }
        tokio::time::sleep(auth_delay(failures)).await;
    }
    command_loop(&mut reader, &mut writer, commands).await
}

/// Authenticated command loop: forward to the tick loop, chunk the reply.
async fn command_loop(
    reader: &mut (impl AsyncReadExt + Unpin),
    writer: &mut (impl AsyncWriteExt + Unpin),
    commands: &tokio::sync::mpsc::Sender<RconRequest>,
) -> mc_core::error::ServerResult<()> {
    loop {
        let packet = decode_body(&read_body(reader).await?)?;
        if packet.kind != TYPE_COMMAND {
            return Err(mc_core::error::ServerError::Protocol(format!(
                "rcon packet has unexpected type {} after login",
                packet.kind
            )));
        }
        let text = String::from_utf8(packet.payload).map_err(|_| {
            mc_core::error::ServerError::Protocol("rcon command is not UTF-8".to_owned())
        })?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        commands
            .send(RconRequest {
                command: text,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                mc_core::error::ServerError::Operational(
                    "the game is gone; rcon cannot run commands".to_owned(),
                )
            })?;
        let reply = reply_rx.await.map_err(|_| {
            mc_core::error::ServerError::Operational("the game dropped an rcon command".to_owned())
        })?;
        for chunk in encode_response(packet.id, &reply) {
            write_all(writer, &chunk).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_AUTH_FAILURES, PREAUTH_TIMEOUT, RconLimits, serve_connection, serve_listener};
    use mc_server::rcon::{
        AUTH_FAILURE_ID, AuthBudget, RconRequest, TYPE_COMMAND, TYPE_LOGIN, TYPE_RESPONSE,
        decode_body, encode_packet, listener_gate,
    };
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// The peer an in-memory (`duplex`) connection pretends to come from.
    const PEER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40_000);

    /// Read one length-prefixed reply body: (id, kind, payload).
    async fn read_packet(reader: &mut (impl AsyncReadExt + Unpin)) -> (i32, i32, Vec<u8>) {
        let mut len = [0u8; 4];
        reader.read_exact(&mut len).await.expect("length");
        let len = usize::try_from(i32::from_le_bytes(len)).expect("test lengths are small");
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await.expect("body");
        let packet = decode_body(&body).expect("decodes");
        (packet.id, packet.kind, packet.payload)
    }

    /// Bind an ephemeral loopback listener and serve it with `limits`.
    ///
    /// Real sockets, because the budgets under test live in `accept`: a duplex
    /// pair has no peer address to budget and no listener to refuse.
    async fn start(
        limits: RconLimits,
    ) -> (SocketAddr, Arc<RconLimits>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("bound address");
        let (tx, rx) = tokio::sync::mpsc::channel::<RconRequest>(16);
        let limits = Arc::new(limits);
        let serving = Arc::clone(&limits);
        let task = tokio::spawn(async move {
            // Hold the receiver: an authenticated command must still find a
            // game at the other end for as long as the test runs.
            let _game = rx;
            let _ = serve_listener(listener, b"s3cret".to_vec(), tx, serving).await;
        });
        (addr, limits, task)
    }

    /// Poll `check` until it holds; the accept loop is concurrent with us.
    async fn wait_until(mut check: impl FnMut() -> bool) {
        for _ in 0..300 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the listener never reached the expected state");
    }

    /// Send one wrong password and read the id `-1` answer.
    async fn bad_login(stream: &mut TcpStream) {
        stream
            .write_all(&encode_packet(1, TYPE_LOGIN, b"wrong"))
            .await
            .expect("attempt");
        let (id, kind, _) = read_packet(stream).await;
        assert_eq!(
            (id, kind),
            (AUTH_FAILURE_ID, TYPE_COMMAND),
            "a bad login answers id -1"
        );
    }

    #[tokio::test]
    async fn stock_login_runs_a_command() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<mc_server::rcon::RconRequest>(16);
        let limits = RconLimits::default();
        let server_task = tokio::spawn(async move {
            serve_connection(server, PEER, b"s3cret", &cmd_tx, &limits).await
        });
        let (mut reader, mut writer) = tokio::io::split(client);
        // Login like a stock client: id 1, password, padding.
        writer
            .write_all(&encode_packet(1, TYPE_LOGIN, b"s3cret"))
            .await
            .expect("login");
        let auth = read_packet(&mut reader).await;
        assert_eq!((auth.0, auth.1), (1, TYPE_COMMAND));
        // Command id 2.
        writer
            .write_all(&encode_packet(2, TYPE_COMMAND, b"list"))
            .await
            .expect("command");
        // The server forwarded the text; answer it like the tick loop would.
        let request = tokio::time::timeout(std::time::Duration::from_secs(2), cmd_rx.recv())
            .await
            .expect("forwarded in time")
            .expect("a request arrived");
        assert_eq!(request.command, "list");
        request
            .reply
            .send("There are 1 players".to_owned())
            .expect("reply");
        let out = read_packet(&mut reader).await;
        assert_eq!((out.0, out.1), (2, 0));
        assert_eq!(out.2, b"There are 1 players");
        server_task.abort();
        // Five is the budget the throttle below also pins.
        assert_eq!(MAX_AUTH_FAILURES, 5);
    }

    #[tokio::test]
    async fn bad_password_gets_id_minus_one_then_closes() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<mc_server::rcon::RconRequest>(16);
        let limits = RconLimits::default();
        let server_task = tokio::spawn(async move {
            serve_connection(server, PEER, b"s3cret", &cmd_tx, &limits).await
        });
        let (mut reader, mut writer) = tokio::io::split(client);
        let bad = encode_packet(9, TYPE_LOGIN, b"wrong");
        for _ in 0..MAX_AUTH_FAILURES {
            writer.write_all(&bad).await.expect("attempt");
            let answer = read_packet(&mut reader).await;
            assert_eq!(
                (answer.0, answer.1),
                (super::AUTH_FAILURE_ID, TYPE_COMMAND),
                "every failure answers id -1"
            );
        }
        // The budget is spent: the server closes, so the next read ends
        // *now* (resolved error), not after a silence timeout — a merely
        // silent server would hang this read instead.
        let mut probe = [0u8; 1];
        let closed = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.read_exact(&mut probe),
        )
        .await
        .expect("the close arrives promptly");
        assert!(closed.is_err(), "the connection closes after the budget");
        server_task.abort();
    }

    #[tokio::test]
    async fn rcon_hostile_input_is_refused() {
        // Huge length, negative length, unknown type: the server answers
        // nothing and closes; the client read ends instead of hanging.
        let huge = 1_000_000i32.to_le_bytes().to_vec();
        let negative = (-5i32).to_le_bytes().to_vec();
        let mut unknown = encode_packet(4, TYPE_COMMAND, b"list");
        unknown[8] = 99;
        for hostile in [huge, negative, unknown] {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<mc_server::rcon::RconRequest>(16);
            let limits = RconLimits::default();
            let server_task = tokio::spawn(async move {
                serve_connection(server, PEER, b"s3cret", &cmd_tx, &limits).await
            });
            let (mut reader, mut writer) = tokio::io::split(client);
            writer.write_all(&hostile).await.expect("probe sent");
            let mut probe = [0u8; 4];
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                reader.read_exact(&mut probe),
            )
            .await;
            assert!(
                read.is_err() || read.expect("readable").is_err(),
                "hostile input is refused, never answered"
            );
            server_task.abort();
        }
    }

    #[tokio::test]
    async fn surplus_preauth_connections_are_refused_not_spawned() {
        // AUDIT-19 A-05/C19-M1: N+1 pre-auth sockets from one address — the
        // surplus is closed at `accept` and never spends budget or a task.
        let (addr, limits, task) = start(RconLimits::default()).await;
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let cap = mc_server::rcon::LISTENER_MAX_CONNECTIONS_PER_IP;
        let mut held = Vec::new();
        for _ in 0..cap {
            held.push(TcpStream::connect(addr).await.expect("connect"));
        }
        // Wait for the listener to account for the whole per-address budget,
        // so what follows is about the surplus and not a race with `accept`.
        wait_until(|| limits.gate.concurrent_for(ip) == cap).await;

        let mut surplus = TcpStream::connect(addr).await.expect("connect");
        let mut probe = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), surplus.read(&mut probe))
            .await
            .expect("the refusal arrives promptly");
        assert!(
            matches!(read, Ok(0)),
            "the surplus socket is closed without being served: {read:?}"
        );
        assert_eq!(
            limits.gate.concurrent_for(ip),
            cap,
            "the refused socket never took a slot"
        );
        drop(held);
        task.abort();
    }

    #[tokio::test]
    async fn an_idle_preauth_socket_is_closed() {
        // AUDIT-19 A-05/C19-M1: `accept` used to hand an unbounded, timeless
        // socket to a task. Silence now has a deadline, whether the client
        // sent nothing at all or stalled halfway through a header.
        assert_eq!(RconLimits::default().preauth_timeout, PREAUTH_TIMEOUT);
        let (addr, _limits, task) = start(RconLimits {
            preauth_timeout: Duration::from_millis(150),
            ..RconLimits::default()
        })
        .await;
        let mut probe = [0u8; 1];
        let mut silent = TcpStream::connect(addr).await.expect("connect");
        let read = tokio::time::timeout(Duration::from_secs(3), silent.read(&mut probe))
            .await
            .expect("the listener closes an idle pre-auth socket");
        assert_eq!(
            read.expect("clean close"),
            0,
            "an idle pre-auth socket ends with EOF"
        );

        let mut stalled = TcpStream::connect(addr).await.expect("connect");
        stalled.write_all(&[2, 0]).await.expect("half a prefix");
        let read = tokio::time::timeout(Duration::from_secs(3), stalled.read(&mut probe))
            .await
            .expect("the listener closes a stalled pre-auth socket");
        assert_eq!(
            read.expect("clean close"),
            0,
            "a partial header is not a licence to stall"
        );
        task.abort();
    }

    #[tokio::test]
    async fn reconnecting_does_not_reset_the_bad_login_budget() {
        // AUDIT-19 C19-M2: three bad logins from one address, spread over two
        // connections, must spend the shared budget — a per-connection counter
        // would forget the first two the moment the client reconnected.
        let (addr, limits, task) = start(RconLimits {
            auth: Arc::new(AuthBudget::new(3, Duration::from_secs(60))),
            ..RconLimits::default()
        })
        .await;
        let mut first = TcpStream::connect(addr).await.expect("connect");
        bad_login(&mut first).await;
        bad_login(&mut first).await;
        drop(first);

        let mut second = TcpStream::connect(addr).await.expect("connect");
        bad_login(&mut second).await;
        let mut probe = [0u8; 4];
        let closed = tokio::time::timeout(Duration::from_secs(2), second.read_exact(&mut probe))
            .await
            .expect("the shared budget closes the connection promptly");
        assert!(
            closed.is_err(),
            "the third failure ends the reconnect, not the count"
        );

        // A spent address is refused at `accept`: no answer, not even id -1.
        let mut third = TcpStream::connect(addr).await.expect("connect");
        let _ = third
            .write_all(&encode_packet(1, TYPE_LOGIN, b"wrong"))
            .await;
        let read = tokio::time::timeout(Duration::from_secs(2), third.read_exact(&mut probe)).await;
        assert!(
            read.is_err() || read.expect("readable").is_err(),
            "a blocked address is refused, never answered"
        );
        assert!(
            limits
                .auth
                .is_blocked(IpAddr::V4(Ipv4Addr::LOCALHOST), Instant::now()),
            "the address stays blocked inside the window"
        );
        task.abort();
    }

    #[tokio::test]
    async fn a_successful_login_clears_the_address_budget() {
        let (addr, limits, task) = start(RconLimits {
            auth: Arc::new(AuthBudget::new(2, Duration::from_secs(60))),
            ..RconLimits::default()
        })
        .await;
        let mut client = TcpStream::connect(addr).await.expect("connect");
        bad_login(&mut client).await;
        // The real password: auth answers with the request id and the run ends.
        client
            .write_all(&encode_packet(7, TYPE_LOGIN, b"s3cret"))
            .await
            .expect("login");
        let (id, kind, _) = read_packet(&mut client).await;
        assert_eq!((id, kind), (7, TYPE_COMMAND));
        assert_eq!(limits.auth.tracked_ips(), 0, "a real login clears the run");
        drop(client);
        task.abort();
    }

    #[tokio::test]
    async fn the_listener_gate_is_the_documented_one() {
        let gate = listener_gate();
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let held: Vec<_> = (0..mc_server::rcon::LISTENER_MAX_CONNECTIONS_PER_IP)
            .map(|_| {
                Arc::clone(&gate)
                    .try_acquire(ip, now)
                    .expect("within the per-IP cap")
            })
            .collect();
        assert_eq!(
            Arc::clone(&gate).try_acquire(ip, now).err(),
            Some(mc_server::rcon::LimitError::PerIpConcurrent),
            "the listener's budget is the game gate, not a private counter"
        );
        drop(held);
        assert_eq!(gate.concurrent_for(ip), 0);
    }

    /// A body spread over several reads is completed, not refused.
    ///
    /// AUDIT-19 C19-L1: Vanilla reads a request with one 1460-byte read and
    /// closes the socket unless the declared length equals exactly what that
    /// read returned, so a *split* packet is a dead connection. Our reader is
    /// length-prefixed and reassembles; the declared length is still the only
    /// thing believed, and it is capped before allocation. Neutralise
    /// [`length_accepted`] (make it answer `true` always) and the tail of this
    /// test goes red, because the oversized declaration is then read instead
    /// of refused.
    #[tokio::test]
    async fn a_body_split_across_reads_is_reassembled_not_refused() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<mc_server::rcon::RconRequest>(16);
        let limits = RconLimits::default();
        let server_task = tokio::spawn(async move {
            serve_connection(server, PEER, b"s3cret", &cmd_tx, &limits).await
        });
        let (mut reader, mut writer) = tokio::io::split(client);

        // Login in three-byte writes with gaps between them: a reader that
        // demanded one whole read would close here. Vanilla would.
        let login = encode_packet(1, TYPE_LOGIN, b"s3cret");
        for piece in login.chunks(3) {
            writer.write_all(piece).await.expect("login piece");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let (id, kind, _) = read_packet(&mut reader).await;
        assert_eq!(
            (id, kind),
            (1, TYPE_COMMAND),
            "a fragmented login still authenticates"
        );

        // The same for a command body, and the forwarded text is whole.
        let command = encode_packet(2, TYPE_COMMAND, b"list");
        for piece in command.chunks(4) {
            writer.write_all(piece).await.expect("command piece");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let request = tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv())
            .await
            .expect("forwarded in time")
            .expect("a request arrived");
        assert_eq!(
            request.command, "list",
            "the reassembled body is the command"
        );
        request
            .reply
            .send("There are 0 players".to_owned())
            .expect("reply");
        let (id, kind, payload) = read_packet(&mut reader).await;
        assert_eq!((id, kind), (2, TYPE_RESPONSE));
        assert_eq!(payload, b"There are 0 players");

        // An oversized declaration is still refused, not read: the
        // reassembly tolerance above did not widen the cap.
        writer
            .write_all(&1_000_000i32.to_le_bytes())
            .await
            .expect("hostile prefix");
        let mut probe = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut probe)).await;
        // EOF or a read error both mean the connection ended (as does the
        // server never answering at all, which is the failure this asserts).
        let closed = if let Ok(outcome) = read {
            outcome.map_or(true, |bytes| bytes == 0)
        } else {
            panic!("the server answered a hostile prefix with silence");
        };
        assert!(
            closed,
            "a declaration above the cap still ends the connection"
        );
        server_task.abort();
    }
}
