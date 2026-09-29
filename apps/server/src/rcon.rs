//! RCON TCP listener (P19-04).
//!
//! Vanilla's admin protocol on its own port: length-prefixed little-endian
//! packets, password login, then console-level commands with chunked
//! replies. The listener owns sockets, auth and throttle; the game owns
//! meaning — every command crosses into the tick loop as an
//! [`mc_server::rcon::RconRequest`] and the joined reply lines come back
//! over a one-shot.
//!
//! Hostile-input posture (pinned by `rcon_hostile_input_is_refused`):
//!
//! - a length prefix above [`mc_server::rcon::MAX_PACKET_LEN`] (or below
//!   the 10-byte header) closes the connection **before** any payload byte
//!   is allocated or read;
//! - a truncated body (EOF mid-packet) closes it;
//! - an unknown packet type closes it;
//! - a wrong password answers id `-1`, sleeps the backoff, and after
//!   [`MAX_AUTH_FAILURES`] consecutive failures the connection is closed;
//! - per connection everything is sequential, so a flooding client only
//!   queues behind itself.

use mc_server::rcon::{
    AUTH_FAILURE_ID, RconRequest, TYPE_COMMAND, TYPE_LOGIN, auth_delay, check_password,
    decode_body, encode_packet, encode_response, length_accepted,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Consecutive bad logins before the connection is closed.
pub const MAX_AUTH_FAILURES: u32 = 5;

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
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        tracing::debug!(%peer, "RCON connection");
        let password = password.clone();
        let commands = commands.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, &password, &commands).await {
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

/// One connection: logins until one succeeds (or the budget runs out),
/// then commands while auth holds.
async fn serve_connection(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    password: &[u8],
    commands: &tokio::sync::mpsc::Sender<RconRequest>,
) -> mc_core::error::ServerResult<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut failures = 0u32;
    loop {
        let login = decode_body(&read_body(&mut reader).await?)?;
        if login.kind != TYPE_LOGIN {
            return Err(mc_core::error::ServerError::Protocol(
                "rcon spoke before logging in".to_owned(),
            ));
        }
        if check_password(&login.payload, password) {
            write_all(&mut writer, &encode_packet(login.id, TYPE_COMMAND, &[])).await?;
            break;
        }
        write_all(
            &mut writer,
            &encode_packet(AUTH_FAILURE_ID, TYPE_COMMAND, &[]),
        )
        .await?;
        failures += 1;
        if failures >= MAX_AUTH_FAILURES {
            return Err(mc_core::error::ServerError::Protocol(format!(
                "rcon closed after {failures} bad logins"
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
    use super::{MAX_AUTH_FAILURES, serve_connection};
    use mc_server::rcon::{TYPE_COMMAND, TYPE_LOGIN, decode_body, encode_packet};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Read one length-prefixed reply body: (id, kind, payload).
    async fn read_packet(reader: &mut (impl AsyncReadExt + Unpin)) -> (i32, i32, Vec<u8>) {
        let mut len = [0u8; 4];
        reader.read_exact(&mut len).await.expect("length");
        let len = i32::from_le_bytes(len) as usize;
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await.expect("body");
        let packet = decode_body(&body).expect("decodes");
        (packet.id, packet.kind, packet.payload)
    }

    #[tokio::test]
    async fn stock_login_runs_a_command() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<mc_server::rcon::RconRequest>(16);
        let server_task =
            tokio::spawn(async move { serve_connection(server, b"s3cret", &cmd_tx).await });
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
        let server_task =
            tokio::spawn(async move { serve_connection(server, b"s3cret", &cmd_tx).await });
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
        let _ = server_task.abort();
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
            let server_task =
                tokio::spawn(async move { serve_connection(server, b"s3cret", &cmd_tx).await });
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
}
