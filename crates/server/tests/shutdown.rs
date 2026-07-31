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
//! This drives the real binary because that is the only place the signal
//! disposition is observable.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Wait for the process to exit, killing it if it outlives the deadline so a
/// failure is a report rather than a hung suite.
fn wait_with_deadline(child: &mut Child, deadline: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            panic!("server did not exit within {deadline:?} of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sigterm_shuts_the_server_down_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("token");
    std::fs::write(&token, "test-token-0123456789abcdef").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mise-server"))
        .arg("--root")
        .arg(dir.path().join("corpus"))
        .args(["--listen", "127.0.0.1:0"])
        .arg("--token-file")
        .arg(&token)
        .arg("--init")
        .env_remove("ANTHROPIC_API_KEY")
        // tracing_subscriber::fmt writes to stdout, not stderr.
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("mise-server binary runs");

    // Wait until it is actually serving. Before that line the shutdown
    // future has not been installed, and the test would be racing the
    // startup rather than testing the handler.
    let logs = BufReader::new(child.stdout.take().unwrap());
    let mut serving = false;
    for line in logs.lines() {
        let line = line.expect("server logs are readable");
        if line.contains("serving corpus") {
            serving = true;
            break;
        }
    }
    assert!(serving, "server never reported that it was serving");

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(killed.success(), "could not signal the server");

    let status = wait_with_deadline(&mut child, Duration::from_secs(10));
    assert!(
        status.success(),
        "SIGTERM should drain and exit 0, got {status:?} (signal {:?}) — \
         an unhandled SIGTERM means the kernel terminated it mid-request",
        status.signal(),
    );
}
