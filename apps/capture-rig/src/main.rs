//! `capture-rig`: relay a real Minecraft client to a server and trace the conversation (P10-01).
//!
//! ```text
//! capture-rig --listen 127.0.0.1:25566 --upstream 127.0.0.1:25565 --out trace.jsonl [--once]
//! ```
//!
//! Point a real client at `--listen`, then read `--out`. The trace is JSONL, one event per line, and the
//! sessions append, so several runs accumulate in one file. `--once` serves a single connection and exits,
//! which is what a scripted regression session wants.

#![forbid(unsafe_code)]

use mc_capture_rig::serve;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

struct Args {
    listen: SocketAddr,
    upstream: SocketAddr,
    out: PathBuf,
    once: bool,
}

fn usage() -> String {
    "usage: capture-rig --listen <addr> --upstream <addr> --out <file> [--once]".to_owned()
}

fn parse_args() -> Result<Args, String> {
    let mut listen: Option<SocketAddr> = None;
    let mut upstream: Option<SocketAddr> = None;
    let mut out: Option<PathBuf> = None;
    let mut once = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("{flag} needs a value\n{}", usage()))
        };
        match arg.as_str() {
            "--listen" => {
                listen = Some(
                    value("--listen")?
                        .parse()
                        .map_err(|e| format!("--listen: {e}"))?,
                );
            }
            "--upstream" => {
                upstream = Some(
                    value("--upstream")?
                        .parse()
                        .map_err(|e| format!("--upstream: {e}"))?,
                );
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--once" => once = true,
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument {other:?}\n{}", usage())),
        }
    }

    Ok(Args {
        listen: listen.ok_or_else(|| format!("--listen is required\n{}", usage()))?,
        upstream: upstream.ok_or_else(|| format!("--upstream is required\n{}", usage()))?,
        out: out.ok_or_else(|| format!("--out is required\n{}", usage()))?,
        once,
    })
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let listener = match TcpListener::bind(args.listen).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot bind {}: {error}", args.listen);
            std::process::exit(1);
        }
    };
    let bound = listener.local_addr().unwrap_or(args.listen);
    eprintln!(
        "capture-rig listening on {bound} -> {}, trace {}",
        args.upstream,
        args.out.display()
    );

    // Append, so several sessions accumulate in one trace rather than the last one winning.
    let path = args.out.clone();
    let new_sink = move || -> Box<dyn Write + Send> {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => Box::new(file),
            Err(error) => {
                eprintln!("cannot open {}: {error}", path.display());
                // A trace that cannot be written must not silently discard the session under observation,
                // so the rig keeps relaying and the failure is visible on stderr.
                Box::new(std::io::sink())
            }
        }
    };

    if let Err(error) = serve(listener, args.upstream, args.once.then_some(1), new_sink).await {
        eprintln!("capture-rig failed: {error}");
        std::process::exit(1);
    }
}
