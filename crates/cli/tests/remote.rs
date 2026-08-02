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

/// A minimal in-test replica: drive one sync session for `store` against
/// the server, straight through the sans-IO `Peer` over a WebSocket —
/// the same protocol the CLI speaks, without needing a second binary.
fn sync_store(store: &mut Store, url: &str) {
    use futures_util::{SinkExt, StreamExt};
    use mise_store::sync::{Peer, WireMsg};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::protocol::Message;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut request = format!("{url}/sync").into_client_request().unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let mut peer = Peer::start(store, true).unwrap();
        let first = peer.initial_round(store).unwrap();
        ws.send(Message::text(first.to_json())).await.unwrap();
        while let Some(incoming) = ws.next().await {
            match incoming.unwrap() {
                Message::Text(text) => {
                    let msg = WireMsg::from_json(&text).unwrap();
                    match peer.handle(store, &msg).unwrap() {
                        Some(reply) => ws.send(Message::text(reply.to_json())).await.unwrap(),
                        None => break,
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = ws.close(None).await;
    });
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

/// The one state the charter forbids is state that exists only in
/// SQLite. Thread messages travel as their own exchange — a session whose
/// only cargo is a transcript line leaves docs_updated empty — and the
/// export guard used to skip exactly that session, so synced transcripts
/// lived in mise.db and nowhere readable, permanently.
#[test]
fn a_thread_only_sync_exports_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn_server(dir.path());
    let a = dir.path().join("a");
    mise(&a, &["init", "--from", &url, "--token", TOKEN]);

    // A third replica contributes one planning-thread message, nothing else.
    let mut c = Store::create_bare(&dir.path().join("c")).unwrap();
    sync_store(&mut c, &url);
    c.append_thread_message(
        &mise_store::threads::ThreadId::Planning,
        mise_store::threads::Role::User,
        "note from the basement",
        jiff::civil::DateTime::constant(2026, 7, 30, 9, 0, 0, 0),
    )
    .unwrap();
    sync_store(&mut c, &url);

    let out = mise(&a, &["sync"]);
    assert!(out.contains("thread messages in"), "{out}");
    let transcript = std::fs::read_to_string(a.join("export/threads/planning.md"))
        .expect("the transcript is exported, not stranded in SQLite");
    assert!(transcript.contains("note from the basement"), "{transcript}");
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

/// SyncOutcome used to count only what arrived, so a push-only session
/// equalled the default outcome and reported "already in sync" — while
/// the server plainly had the new edits.
#[test]
fn a_push_only_sync_says_pushed_not_already_in_sync() {
    let dir = tempfile::tempdir().unwrap();
    let url = spawn_server(dir.path());
    let a = dir.path().join("a");
    mise(&a, &["init", "--from", &url, "--token", TOKEN]);

    mise(&a, &["pantry", "set", "miso", "--presence", "have"]);
    let out = mise(&a, &["sync"]);
    assert!(!out.contains("already in sync"), "{out}");
    assert!(out.contains("pushed"), "{out}");

    // And once everything has travelled, "already in sync" is true again.
    let out = mise(&a, &["sync"]);
    assert!(out.contains("already in sync"), "{out}");
}

/// #31, at the surface it actually regressed: two devices each add a fridge
/// portion while offline, and both survive the merge. Before the CLI routed
/// through the tool, `mise fridge add` scanned for the lowest free `p<n>` in
/// the local replica, so both picked `p1` and one portion was destroyed on
/// sync. The mint the tool uses is replica-scoped, so the two ids differ.
#[test]
fn cli_fridge_adds_on_two_devices_both_survive_the_merge() {
    use mise_store::DocId;
    use mise_store::pages::FridgeDoc;

    let dir = tempfile::tempdir().unwrap();
    let url = spawn_server(dir.path());
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    mise(&a, &["init", "--from", &url, "--token", TOKEN]);
    mise(&b, &["init", "--from", &url, "--token", TOKEN]);

    // Offline on both — the signal-dead-kitchen case.
    mise(&a, &["fridge", "add", "Sunday mapo", "--servings", "3"]);
    mise(&b, &["fridge", "add", "Chili", "--servings", "4"]);

    mise(&a, &["sync"]);
    mise(&b, &["sync"]);
    mise(&a, &["sync"]);

    let home = Slug::new("home").unwrap();
    let fridge: FridgeDoc = Store::open(&a).unwrap().get(&DocId::Fridge(home)).unwrap();
    let dishes: Vec<_> = fridge.fridge.values().map(|p| p.dish.as_str()).collect();
    assert_eq!(fridge.fridge.len(), 2, "a fridge id collision swallowed a portion: {dishes:?}");
    assert_eq!(export_tree(&a), export_tree(&b));
}

/// #56 / #33 standing guard: the same operation through the `mise` binary and
/// through `tools::execute` must leave identical document state, so the two
/// surfaces cannot drift apart again. Non-minting ops only — a minted id is
/// replica-scoped and so is deliberately *not* identical across corpora.
/// Change timestamps live in Automerge metadata, not the hydrated corpus, so a
/// wall-clock CLI and a fixed-clock tool still compare equal on content.
#[test]
fn cli_and_tool_leave_identical_doc_state() {
    use mise_assistant::tools::{self, ToolCtx};
    use mise_assistant::turn::ToolCall;
    use serde_json::json;

    let cases: &[(&[&str], &str, serde_json::Value)] = &[
        (
            &["pantry", "set", "miso", "--presence", "out", "--tier", "town"],
            "pantry_set",
            json!({"item": "miso", "presence": "out", "tier": "town"}),
        ),
        (
            &["equipment", "add", "wok", "--note", "carbon steel"],
            "equipment_set",
            json!({"item": "wok", "note": "carbon steel"}),
        ),
        (
            &["queue", "add", "Dal", "--reason", "cheap"],
            "queue_add",
            json!({"title": "Dal", "reason": "cheap"}),
        ),
    ];

    for (argv, tool, input) in cases {
        let dir = tempfile::tempdir().unwrap();
        let home = Slug::new("home").unwrap();

        // The CLI arm: a real subprocess into its own corpus.
        let cli_root = dir.path().join("cli");
        mise(&cli_root, &["init", "--location", "home", "--headcount", "2"]);
        mise(&cli_root, argv);

        // The tool arm: the same input straight through tools::execute.
        let tool_root = dir.path().join("tool");
        let mut store =
            Store::create(&tool_root, &home, 2, jiff::Timestamp::now()).unwrap();
        let ctx = ToolCtx { now: jiff::Zoned::now(), provenance: "cli".into() };
        let call = ToolCall { id: "t".into(), name: (*tool).into(), input: input.clone() };
        let out = tools::execute(&mut store, &ctx, &call).unwrap();
        assert!(!out.is_error, "{tool}: {}", out.content);

        let cli = Store::open(&cli_root).unwrap().corpus().unwrap();
        let via_tool = store.corpus().unwrap();
        assert_eq!(cli, via_tool, "CLI and tool diverged on {tool}");
    }
}

/// A server hanging up mid-session — graceful shutdown, proxy idle
/// timeout — used to produce exit 0 and "already in sync". Received data
/// is persisted either way; only the reporting and exit status lie.
#[test]
fn a_sync_cut_off_early_fails_instead_of_reporting_success() {
    use futures_util::StreamExt;

    // A "server" that accepts the handshake, reads one round, hangs up.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = ws.next().await;
            let _ = ws.close(None).await;
        });
    });
    let addr = rx.recv().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("solo");
    mise(&root, &["init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_mise"))
        .arg("--root")
        .arg(&root)
        .args(["sync", "--server", &format!("ws://{addr}/sync"), "--token", TOKEN])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a session cut off early is not a success: {}",
        String::from_utf8_lossy(&out.stdout),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ended early"), "{stderr}");
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
