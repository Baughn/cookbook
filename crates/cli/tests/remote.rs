//! End-to-end remote mode: a real server in-process, two devices driven
//! entirely through the `mise` binary — join, edit offline, sync, converge.

use std::path::Path;
use std::process::Command;

use mise_core::types::Slug;
use mise_server::{AppState, app};
use mise_store::Store;

const TOKEN: &str = "test-token-0123456789abcdef";

fn mise(root: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mise {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Serve a fresh corpus on an ephemeral port from a background thread.
fn spawn_server(dir: &Path) -> String {
    let mut store = Store::create(&dir.join("server"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::now()).unwrap();
    store.export("init: empty corpus").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app(AppState::new(store, TOKEN.to_string())))
                .await
                .unwrap();
        });
    });
    let addr = rx.recv().unwrap();
    format!("ws://{addr}")
}

/// Export trees (path → bytes), .git excluded.
fn export_tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&root.join("export"), &root.join("export"), &mut out);
    out
}

#[test]
fn devices_join_edit_offline_and_converge() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn_server(dir.path());
    let a = dir.path().join("a");
    let b = dir.path().join("b");

    // Join from the server; the remote is remembered.
    let out = mise(&a, &["init", "--from", &url, "--token", TOKEN]);
    assert!(out.contains("joined corpus"), "{out}");
    mise(&b, &["init", "--from", &url, "--token", TOKEN]);
    let out = mise(&a, &["remote", "show"]);
    assert!(out.contains("/sync"), "{out}");

    // Offline edits on both devices: A stocks the pantry and logs a cook,
    // B queues a dish.
    mise(&a, &["pantry", "set", "miso", "--presence", "have", "--tier", "town"]);
    mise(&a, &["log", "add", "Mapo tofu", "--servings", "4", "--verdict", "great"]);
    mise(&b, &["queue", "add", "Duck curry", "--reason", "basement idea"]);

    // Everyone syncs; A twice to see B's edits.
    let out = mise(&a, &["sync"]);
    assert!(out.contains("log entries out") || out.contains("updated"), "{out}");
    mise(&b, &["sync"]);
    mise(&a, &["sync"]);

    // Converged: same queue view, byte-identical exports on both devices
    // and on the server.
    let qa = mise(&a, &["queue"]);
    let qb = mise(&b, &["queue"]);
    assert_eq!(qa, qb);
    assert!(qa.contains("Duck curry"), "{qa}");
    assert_eq!(export_tree(&a), export_tree(&b));
    assert_eq!(export_tree(&a), export_tree(&dir.path().join("server")));

    // A third sync is a no-op.
    let out = mise(&a, &["sync"]);
    assert!(out.contains("already in sync"), "{out}");
}

/// A join whose first sync fails (bad token, server down) leaves a bare
/// corpus with no documents. That must be retryable — `init --from` again
/// with the problem fixed — not a dead end where every command says
/// "no such document: state" and re-init says "already initialized".
#[test]
fn a_failed_join_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn_server(dir.path());
    let root = dir.path().join("c");

    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .arg("--root")
        .arg(&root)
        .args(["init", "--from", &url, "--token", "wrong-token-9876543210zyxwvu"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a bad token must fail the join");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("init --from"), "the error names the recovery: {stderr}");

    // Same command, right token: the half-joined root is picked up and
    // the first sync retried.
    let out = mise(&root, &["init", "--from", &url, "--token", TOKEN]);
    assert!(out.contains("joined corpus"), "{out}");
    mise(&root, &["queue"]);
}

#[test]
fn sync_without_remote_fails_helpfully() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("solo");
    mise(&root, &["init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .arg("--root")
        .arg(&root)
        .arg("sync")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("mise remote set"), "{stderr}");
}
