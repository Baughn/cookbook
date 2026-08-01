//! Shutdown, against the signal that actually arrives in production.
//!
//! systemd's default `KillSignal` is SIGTERM and the unit sets no override,
//! so a handler for SIGINT alone is a graceful-shutdown path that never runs
//! where it matters: the kernel default terminates the process outright,
//! cutting in-flight sync sockets and SSE streams mid-frame. The worst case
//! is a restart landing inside `Store::export`, which rewrites and removes
//! files before `git add`/`commit` — leaving the readable backup
//! half-rewritten and uncommitted.
//!
//! This drives the real binary, because a signal disposition is not
//! observable any other way.
//!
//! Readiness is a TCP connect, not a log line. The first version of this
//! test waited for "serving corpus" on stdout and hung for twelve hours
//! inside a `nix build` sandbox, where `RUST_LOG` suppressed every event:
//! the server was up and listening the whole time, silently. Logging is
//! configuration; a listening socket is the thing being tested.

#![cfg(unix)]

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Long enough for a cold start under a loaded builder, short enough that a
/// hang is a failing test rather than a stuck suite. Every wait in here is
/// bounded — that is the lesson from the version that wasn't.
const PATIENCE: Duration = Duration::from_secs(30);

/// A free loopback port. Racy in principle — the port could be taken between
/// the drop and the server's bind — but the alternative is `:0`, whose port
/// only the server knows, and it would have to tell us through a channel
/// that works when logging is off.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Everything the server wrote, for failure messages.
fn logs(path: &Path) -> String {
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    if s.trim().is_empty() { "<no output>".to_string() } else { s }
}

/// Wait until the server accepts a connection, failing fast if it exits
/// first — an early exit is a startup error worth reporting, not a timeout.
fn wait_until_serving(child: &mut Child, addr: SocketAddr, log: &Path) {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("server exited before serving: {status:?}\n{}", logs(log));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        if start.elapsed() > PATIENCE {
            let _ = child.kill();
            panic!("server was not accepting connections within {PATIENCE:?}\n{}", logs(log));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_exit(child: &mut Child, log: &Path) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > PATIENCE {
            let _ = child.kill();
            panic!("server did not exit within {PATIENCE:?} of SIGTERM\n{}", logs(log));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn sigterm_shuts_the_server_down_gracefully() {
    use std::os::unix::process::ExitStatusExt;

    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    let log = dir.path().join("server.log");
    std::fs::write(&token, "test-token-0123456789abcdef").unwrap();

    let addr: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    let out = std::fs::File::create(&log).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mise-server"))
        .arg("--root")
        .arg(dir.path().join("corpus"))
        .arg("--listen")
        .arg(addr.to_string())
        .arg("--token-file")
        .arg(&token)
        .arg("--init")
        .env_remove("ANTHROPIC_API_KEY")
        .stdout(Stdio::from(out.try_clone().unwrap()))
        .stderr(Stdio::from(out))
        .spawn()
        .expect("mise-server binary runs");

    wait_until_serving(&mut child, addr, &log);

    // SIGTERM, not Child::kill — that sends SIGKILL, which no handler can
    // catch and which would make this test pass against the bug.
    let sent = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(sent, 0, "could not signal the server");

    let status = wait_for_exit(&mut child, &log);
    assert!(
        status.success(),
        "SIGTERM should drain and exit 0, got {status:?} (signal {:?}) — \
         an unhandled SIGTERM means the kernel terminated it mid-request\n{}",
        status.signal(),
        logs(&log),
    );
}
