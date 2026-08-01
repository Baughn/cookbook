//! Replica sync at the store level, no transport: two real stores in temp
//! dirs shuttle wire messages until done. Two clients converging — offline
//! edits included — is the M2 promise; converged stores must also render
//! byte-identical exports.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use jiff::civil::{Date, DateTime};
use mise_core::types::{CookKind, LogEntry, Slug};
use mise_store::pages::{
    DishRefDoc, PantryDoc, PantryItemDoc, QueueDoc, QueueEntryDoc, RecipeDoc, StateDoc,
};
use mise_store::render::render;
use mise_store::sync::{Peer, SyncOutcome};
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};
use proptest::collection::vec;
use proptest::prelude::*;

fn t0() -> jiff::Timestamp {
    jiff::Timestamp::UNIX_EPOCH
}

fn slug(s: &str) -> Slug {
    Slug::new(s).unwrap()
}

/// The documented uid prefix: `sha256(serialized content)[..16]` — computed
/// from scratch here so these tests pin the wire contract, not the
/// implementation's helper.
fn content_hash<T: serde::Serialize>(value: &T) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(serde_json::to_string(value).unwrap().as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Drive a full sync session between two stores, no transport.
fn run_sync(a: &mut Store, b: &mut Store) -> (SyncOutcome, SyncOutcome) {
    let mut pa = Peer::start(a, true).unwrap();
    let mut pb = Peer::start(b, false).unwrap();
    let mut msg = pa.initial_round(a).unwrap();
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 64, "sync did not terminate");
        let reply = pb.handle(b, &msg).unwrap().expect("responder always replies");
        match pa.handle(a, &reply).unwrap() {
            Some(next) => msg = next,
            None => break,
        }
    }
    (pa.outcome().clone(), pb.outcome().clone())
}

fn exports_equal(a: &Store, b: &Store) -> bool {
    render(&a.corpus().unwrap()) == render(&b.corpus().unwrap())
}

fn create_at(dir: &Path, name: &str) -> Store {
    Store::create(&dir.join(name), &slug("home"), 2, t0()).unwrap()
}

#[test]
fn fresh_replica_pulls_everything() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    a.modify::<PantryDoc>(&DocId::Pantry(slug("home")), "seed", t0(), |p| {
        p.items.insert(
            "miso".into(),
            PantryItemDoc {
                name: "miso".into(),
                presence: "have".into(),
                bought: None,
                tier: Some("town".into()),
                note: None,
            },
        );
    })
    .unwrap();
    a.append_log(&LogEntry {
        date: Date::constant(2026, 7, 28),
        kind: CookKind::Meal,
        recipe: None,
        title: "Mapo tofu".into(),
        location: "home".into(),
        servings: 4,
        verdict: "great".into(),
        tags: BTreeMap::new(),
    }, "test: log", t0())
    .unwrap();

    // The second device starts with nothing at all.
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    let (out_a, out_b) = run_sync(&mut a, &mut b);

    assert!(out_a.docs_updated.is_empty(), "{out_a:?}");
    assert_eq!(out_a.log_sent, 1);
    assert!(out_b.docs_updated.contains("location/home/pantry"));
    assert_eq!(out_b.log_added, 1);
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());
    assert!(exports_equal(&a, &b));
}

#[test]
fn offline_edits_converge_and_resync_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    // Both sides edit offline: a new recipe on A, queue + pantry on B.
    a.create_doc(
        &DocId::Recipe(slug("duck-curry")),
        &RecipeDoc {
            schema_version: 1,
            title: "Duck curry".into(),
            servings: 4,
            effort: "project".into(),
            lead: None,
            tags: BTreeMap::from([("protein".to_string(), "duck".to_string())]),
            equipment: vec![],
            ingredients: vec![],
            source: None,
            status: "active".into(),
            body: "Brown the legs.".into(),
        },
        "offline on a",
    t0(),
    )
    .unwrap();
    b.modify::<QueueDoc>(&DocId::Queue, "offline on b", t0(), |q| {
        q.entries.insert(
            "duck-curry".into(),
            QueueEntryDoc {
                dishes: vec![DishRefDoc { recipe: Some("duck-curry".into()), title: "Duck curry".into() }],
                reason: Some("uses the duck legs".into()),
                added: "2026-07-29".into(),
            },
        );
    })
    .unwrap();
    b.modify::<PantryDoc>(&DocId::Pantry(slug("home")), "offline on b", t0(), |p| {
        p.items.insert(
            "duck-legs".into(),
            PantryItemDoc {
                name: "duck legs".into(),
                presence: "have".into(),
                bought: Some("2026-07-29".into()),
                tier: Some("butcher".into()),
                note: None,
            },
        );
    })
    .unwrap();

    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert!(out_a.docs_updated.contains("queue"), "{out_a:?}");
    assert!(out_a.docs_updated.contains("location/home/pantry"));
    assert!(out_b.docs_updated.contains("recipe/duck-curry"), "{out_b:?}");
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());
    assert!(exports_equal(&a, &b));

    // Nothing left to say: a second session moves no data.
    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert!(out_a.is_empty(), "{out_a:?}");
    assert!(out_b.is_empty(), "{out_b:?}");
}

/// Log-row identity is (content, minting replica, occurrence): a row
/// reaches every device exactly once, and genuinely repeated identical
/// cooks stay distinct even when the repeats straddle a partition. The
/// occurrence index counts only the minting replica's own repeats, so two
/// devices that each log the same dish twice while apart converge to four
/// cooks — the old replica-blind count collapsed them to two.
#[test]
fn partitioned_repeat_cooks_all_survive_the_merge() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let entry = LogEntry {
        date: Date::constant(2026, 7, 29),
        kind: CookKind::Meal,
        recipe: None,
        title: "Mapo tofu".into(),
        location: "home".into(),
        servings: 4,
        verdict: "fine".into(),
        tags: BTreeMap::new(),
    };
    // Partitioned: each device logs the same dish twice before they meet.
    let a0 = a.append_log(&entry, "test: log", t0()).unwrap();
    let a1 = a.append_log(&entry, "test: log", t0()).unwrap();
    let b0 = b.append_log(&entry, "test: log", t0()).unwrap();

    // Same content, same device → the occurrence index disambiguates.
    assert!(a0.ends_with("-0"), "{a0}");
    assert!(a1.ends_with("-1"), "{a1}");
    assert_eq!(a0.rsplit_once('-').unwrap().0, a1.rsplit_once('-').unwrap().0);
    // Same content, different device → different replica component, shared
    // content-hash prefix.
    assert_ne!(a0, b0);
    assert_eq!(a0.split('-').next(), b0.split('-').next(), "same content hash");

    run_sync(&mut a, &mut b);
    assert_eq!(a.log_entries().unwrap().len(), 3, "every distinct cook survives");
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());
    assert!(exports_equal(&a, &b));

    // Idempotent: meeting again moves nothing.
    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert!(out_a.is_empty(), "{out_a:?}");
    assert!(out_b.is_empty(), "{out_b:?}");
}

/// The uid is the entire cross-replica identity of a log row, so sync must
/// not take a peer's word for it: a forged uid could shadow the genuine row
/// forever (`INSERT OR IGNORE` on the real uid would swallow the real
/// entry). The round is rejected whole — nothing in it persists.
#[test]
fn a_log_row_whose_uid_does_not_match_its_content_poisons_the_round() {
    use mise_store::sync::{LogRow, Round, WireMsg};

    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let entry = LogEntry {
        date: Date::constant(2026, 7, 29),
        kind: CookKind::Meal,
        recipe: None,
        title: "Mapo tofu".into(),
        location: "home".into(),
        servings: 4,
        verdict: "fine".into(),
        tags: BTreeMap::new(),
    };
    let round = Round {
        log_entries: vec![
            // A well-formed row under an honest uid…
            LogRow { uid: format!("{}-0", content_hash(&entry)), entry: entry.clone() },
            // …and one whose uid belongs to no such content.
            LogRow { uid: "0000000000000000-0".into(), entry: entry.clone() },
        ],
        ..Round::default()
    };

    let mut pb = Peer::start(&b, false).unwrap();
    pb.handle(&mut b, &WireMsg::Round(round)).expect_err("a forged uid rejects the round");
    assert_eq!(b.log_entries().unwrap().len(), 0, "nothing from the poisoned round persisted");
}

/// Thread content is normalized on append (LF, trimmed, non-empty) and the
/// renderer depends on it — so the sync path must apply the same
/// normalization before hashing and storing, or identical turns get
/// different uids across replicas and raw CRLF reaches the renderer.
#[test]
fn sync_normalizes_thread_content_before_verifying_and_storing_it() {
    use mise_store::sync::{Round, ThreadRow, WireMsg};
    use mise_store::threads::ThreadMessage;

    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let normalized = ThreadMessage {
        thread: ThreadId::Planning,
        role: Role::User,
        content: "plan the week".into(),
        created: DateTime::constant(2026, 7, 29, 9, 0, 0, 0),
    };
    let raw = ThreadMessage { content: "plan the week\r\n ".into(), ..normalized.clone() };
    // An honest peer hashes the normalized form; the wire carries the raw one.
    let uid = format!("{}-0", content_hash(&normalized));
    let round = Round {
        thread_entries: vec![ThreadRow { uid, message: raw }],
        ..Round::default()
    };

    let mut pb = Peer::start(&b, false).unwrap();
    pb.handle(&mut b, &WireMsg::Round(round)).unwrap();
    let messages = b.thread_messages(&ThreadId::Planning).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "plan the week", "stored normalized, not raw");

    // A message that normalizes to nothing rejects the round.
    let empty = ThreadMessage { content: " \r\n ".into(), ..normalized.clone() };
    let round = Round {
        thread_entries: vec![ThreadRow { uid: format!("{}-0", content_hash(&empty)), message: empty }],
        ..Round::default()
    };
    let mut pb = Peer::start(&b, false).unwrap();
    pb.handle(&mut b, &WireMsg::Round(round)).expect_err("an empty message rejects the round");
}

#[test]
fn thread_messages_sync_across_devices() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let at = |h: i8| DateTime::constant(2026, 7, 29, h, 0, 0, 0);
    // A planning exchange on A, a page-thread question on B…
    a.append_thread_message(&ThreadId::Planning, Role::User, "plan the week", at(9))
        .unwrap();
    a.append_thread_message(&ThreadId::Planning, Role::Assistant, "three dishes: …", at(9))
        .unwrap();
    let recipe_thread = ThreadId::Page(DocId::Recipe(slug("mapo-tofu")));
    b.append_thread_message(&recipe_thread, Role::User, "can I halve the sugar?", at(10))
        .unwrap();
    // …and one message typed identically on both devices. Identity is
    // (content, minting replica, occurrence), so these are two messages —
    // each device said it once, and both records survive the merge.
    for s in [&mut a, &mut b] {
        s.append_thread_message(&ThreadId::Planning, Role::User, "checked off eggs", at(11))
            .unwrap();
    }

    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert_eq!(out_a.threads_sent, 3, "{out_a:?}");
    assert_eq!(out_b.threads_sent, 2, "{out_b:?}");
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());
    assert!(exports_equal(&a, &b));
    assert_eq!(a.thread_messages(&ThreadId::Planning).unwrap().len(), 4);

    // A row reaches every device exactly once: resync moves nothing.
    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert!(out_a.is_empty(), "{out_a:?}");
    assert!(out_b.is_empty(), "{out_b:?}");
}

// ------------------------------------------------------------ properties --

#[derive(Clone, Debug)]
enum Op {
    Pantry { k: u8, presence: u8 },
    Queue { k: u8, title: String },
    Log { day: u8, title: String },
    Thread { k: u8, text: String },
}

fn arb_op() -> impl Strategy<Value = Op> {
    let word = || proptest::string::string_regex("[a-z]{1,8}").unwrap();
    prop_oneof![
        (any::<u8>(), any::<u8>()).prop_map(|(k, presence)| Op::Pantry { k, presence }),
        (any::<u8>(), word()).prop_map(|(k, title)| Op::Queue { k, title }),
        (any::<u8>(), word()).prop_map(|(day, title)| Op::Log { day, title }),
        (any::<u8>(), word()).prop_map(|(k, text)| Op::Thread { k, text }),
    ]
}

fn apply(store: &mut Store, op: &Op) {
    match op {
        Op::Pantry { k, presence } => {
            store
                .modify::<PantryDoc>(&DocId::Pantry(slug("home")), "prop", t0(), |p| {
                    let key = format!("item{}", k % 5);
                    p.items.insert(
                        key.clone(),
                        PantryItemDoc {
                            name: key,
                            presence: ["have", "low", "out"][(*presence % 3) as usize].into(),
                            bought: None,
                            tier: None,
                            note: None,
                        },
                    );
                })
                .unwrap();
        }
        Op::Queue { k, title } => {
            store
                .modify::<QueueDoc>(&DocId::Queue, "prop", t0(), |q| {
                    q.entries.insert(
                        format!("q{}", k % 5),
                        QueueEntryDoc {
                            dishes: vec![DishRefDoc { recipe: None, title: title.clone() }],
                            reason: None,
                            added: "2026-07-29".into(),
                        },
                    );
                })
                .unwrap();
        }
        Op::Log { day, title } => {
            store
                .append_log(&LogEntry {
                    date: Date::constant(2026, 7, i8::try_from(1 + day % 28).unwrap()),
                    kind: CookKind::Meal,
                    recipe: None,
                    title: title.clone(),
                    location: "home".into(),
                    servings: 2,
                    verdict: "fine".into(),
                    tags: BTreeMap::new(),
                }, "test: log", t0())
                .unwrap();
        }
        Op::Thread { k, text } => {
            let thread = if k % 2 == 0 {
                ThreadId::Planning
            } else {
                ThreadId::Page(DocId::Queue)
            };
            let role = if k % 4 < 2 { Role::User } else { Role::Assistant };
            store
                .append_thread_message(
                    &thread,
                    role,
                    text,
                    DateTime::constant(2026, 7, 29, i8::try_from(k % 24).unwrap(), 0, 0, 0),
                )
                .unwrap();
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

    /// Arbitrary divergent edits on two synced replicas: one session
    /// reconverges them, states equal, exports byte-identical.
    #[test]
    fn divergent_stores_reconverge(
        ops_a in vec(arb_op(), 0..8),
        ops_b in vec(arb_op(), 0..8),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut a = create_at(dir.path(), "a");
        let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
        run_sync(&mut a, &mut b);

        for op in &ops_a {
            apply(&mut a, op);
        }
        for op in &ops_b {
            apply(&mut b, op);
        }
        run_sync(&mut a, &mut b);

        prop_assert_eq!(a.corpus().unwrap(), b.corpus().unwrap());
        prop_assert!(exports_equal(&a, &b));

        let (out_a, out_b) = run_sync(&mut a, &mut b);
        prop_assert!(out_a.is_empty(), "{:?}", out_a);
        prop_assert!(out_b.is_empty(), "{:?}", out_b);
    }
}

// ------------------------------------------------------------- migration --

/// The M1 (v1) schema, verbatim, so migration stays honest about its past.
const SCHEMA_V1: &str = "
CREATE TABLE docs (
  id   TEXT PRIMARY KEY,
  kind TEXT NOT NULL
) STRICT;
CREATE TABLE doc_changes (
  doc_id TEXT NOT NULL REFERENCES docs(id),
  seq    INTEGER NOT NULL,
  change BLOB NOT NULL,
  PRIMARY KEY (doc_id, seq)
) STRICT;
CREATE TABLE doc_snapshots (
  doc_id   TEXT NOT NULL REFERENCES docs(id),
  upto_seq INTEGER NOT NULL,
  snapshot BLOB NOT NULL,
  PRIMARY KEY (doc_id, upto_seq)
) STRICT;
CREATE TABLE cook_log (
  id       INTEGER PRIMARY KEY,
  date     TEXT NOT NULL,
  kind     TEXT NOT NULL,
  recipe   TEXT,
  title    TEXT NOT NULL,
  location TEXT NOT NULL,
  servings INTEGER NOT NULL,
  verdict  TEXT NOT NULL,
  tags     TEXT NOT NULL
) STRICT;
CREATE TABLE threads (
  id   INTEGER PRIMARY KEY,
  page TEXT NOT NULL
) STRICT;
CREATE TABLE thread_messages (
  id        INTEGER PRIMARY KEY,
  thread_id INTEGER NOT NULL REFERENCES threads(id),
  role      TEXT NOT NULL,
  content   TEXT NOT NULL,
  created   TEXT NOT NULL
) STRICT;
CREATE TABLE blobs (
  hash TEXT PRIMARY KEY,
  ext  TEXT NOT NULL
) STRICT;
PRAGMA user_version = 1;
";

#[test]
fn v1_corpus_migrates_on_open() {
    use automerge::AutoCommit;
    use autosurgeon::reconcile;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    std::fs::create_dir_all(root.join("export")).unwrap();

    // Fabricate a v1 database with one real doc change and two log rows,
    // one of them a duplicated cook.
    {
        let conn = rusqlite::Connection::open(root.join("mise.db")).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        let mut doc = AutoCommit::new();
        reconcile(&mut doc, StateDoc::new("home", 2)).unwrap();
        doc.commit();
        let change = doc.get_last_local_change().unwrap().raw_bytes().to_vec();
        conn.execute("INSERT INTO docs (id, kind) VALUES ('state', 'state')", []).unwrap();
        conn.execute(
            "INSERT INTO doc_changes (doc_id, seq, change) VALUES ('state', 1, ?1)",
            rusqlite::params![change],
        )
        .unwrap();
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO cook_log (date, kind, recipe, title, location, servings, verdict, tags)
                 VALUES ('2026-07-28', 'meal', NULL, 'Mapo tofu', 'home', 4, 'fine', '{}')",
                [],
            )
            .unwrap();
        }
        std::process::Command::new("git")
            .args(["-C"])
            .arg(root.join("export"))
            .args(["init", "-q"])
            .output()
            .unwrap();
    }

    let store = Store::open(&root).unwrap();
    let state: StateDoc = store.get(&DocId::State).unwrap();
    assert_eq!(state.active_location, "home");
    assert_eq!(store.log_entries().unwrap().len(), 2);

    // Backfill happened: hashes and distinct uids exist.
    let conn = rusqlite::Connection::open(root.join("mise.db")).unwrap();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 5);
    let uids: Vec<String> = conn
        .prepare("SELECT uid FROM cook_log ORDER BY uid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(uids.len(), 2);
    assert_ne!(uids[0], uids[1], "occurrence index disambiguates");
    assert_eq!(uids[0].split('-').next(), uids[1].split('-').next(), "same content hash");
    let null_hashes: i64 = conn
        .query_row("SELECT COUNT(*) FROM doc_changes WHERE hash IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(null_hashes, 0);
}

// ------------------------------------------------------- schema on the wire --

/// Sync is where a doc shape crosses a build boundary: the CRDT layer merges
/// changes without inspecting them, so an un-upgraded phone reintroduces an
/// old shape whenever it syncs. That is why shape changes ship a permanent
/// tolerant hydrator rather than a converter, and why the wire says which
/// shape the changes in a session were written at.
#[test]
fn both_sides_announce_the_shape_they_write() {
    use mise_store::sync::{Round, WireMsg};

    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();

    // The opening round carries it, and so does the responder's first reply.
    let mut pa = Peer::start(&a, true).unwrap();
    let mut pb = Peer::start(&b, false).unwrap();
    let opening = pa.initial_round(&a).unwrap();
    let WireMsg::Round(round) = &opening else { panic!("opening is a round") };
    assert_eq!(round.schema, Some(mise_store::pages::SCHEMA_VERSION));
    assert!(opening.to_json().contains(r#""schema":1"#), "{}", opening.to_json());

    let reply = pb.handle(&mut b, &opening).unwrap().unwrap();
    let WireMsg::Round(round) = &reply else { panic!("responder replies with a round") };
    assert_eq!(round.schema, Some(mise_store::pages::SCHEMA_VERSION));

    // Announced once per side, not in every round.
    let second = pa.handle(&mut a, &reply).unwrap().unwrap();
    if let WireMsg::Round(round) = &second {
        assert_eq!(round.schema, None, "the shape is announced once, with the uids");
    }

    // Both sides learned the other's.
    assert_eq!(pb.outcome().peer_schema, Some(mise_store::pages::SCHEMA_VERSION));
    assert!(!pb.outcome().peer_is_newer());

    // A round with no schema field at all is a peer from before it existed,
    // which is version 1 — not "unknown", and not a reason to refuse it.
    let mut pc = Peer::start(&b, false).unwrap();
    let legacy = WireMsg::Round(Round { schema: None, ..Round::default() });
    pc.handle(&mut b, &legacy).unwrap();
    assert_eq!(pc.outcome().peer_schema, Some(mise_store::sync::SCHEMA_ABSENT));
    assert!(!pc.outcome().peer_is_newer());
}

/// A peer running a build ahead of this one still gets its changes applied —
/// refusing them would drop exactly the offline edits sync exists to carry.
/// The outcome says so instead, so the surface can tell the user to upgrade.
#[test]
fn a_newer_peer_is_reported_not_refused() {
    use mise_store::sync::{Round, WireMsg};

    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    let newer = mise_store::pages::SCHEMA_VERSION + 1;

    // A full session with every announcement from A relabelled as a build
    // ahead of this one. Automerge's opening message carries heads, not
    // changes, so the data only moves once the session runs to completion.
    let relabel = |msg: WireMsg| match msg {
        WireMsg::Round(round) if round.schema.is_some() => {
            WireMsg::Round(Round { schema: Some(newer), ..round })
        }
        other => other,
    };
    let mut pa = Peer::start(&a, true).unwrap();
    let mut pb = Peer::start(&b, false).unwrap();
    let mut msg = relabel(pa.initial_round(&a).unwrap());
    loop {
        let reply = pb.handle(&mut b, &msg).unwrap().expect("responder still replies");
        match pa.handle(&mut a, &reply).unwrap() {
            Some(next) => msg = relabel(next),
            None => break,
        }
    }

    assert_eq!(pb.outcome().peer_schema, Some(newer));
    assert!(pb.outcome().peer_is_newer());
    assert!(!pb.outcome().docs_updated.is_empty(), "its changes were applied anyway");
    assert_eq!(a.corpus().unwrap(), b.corpus().unwrap(), "and they converged");
}

/// `is_empty` is not `== default()`: learning the peer's shape is not data
/// moving, so an idempotent re-sync still reads as empty.
#[test]
fn an_idempotent_resync_is_empty_even_though_it_learned_the_shape() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let (out_a, out_b) = run_sync(&mut a, &mut b);
    assert!(out_a.is_empty(), "{out_a:?}");
    assert!(out_b.is_empty(), "{out_b:?}");
    assert_eq!(out_a.peer_schema, Some(mise_store::pages::SCHEMA_VERSION));
    assert_ne!(out_a, SyncOutcome::default(), "learning it is still recorded");
}

/// The motivating disaster for stale snapshots: a sync session is open (a
/// phone in the doorway) while another surface edits the pantry — the
/// server holds its store lock only around individual calls, so this
/// interleaving is routine. When the session's incoming changes cross the
/// snapshot cadence, the snapshot must describe what the rows say, not the
/// session's stale in-memory doc — or the concurrent edit becomes invisible
/// to every read AND to every future sync, silently.
#[test]
fn a_mid_session_edit_survives_the_snapshot_cadence() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = create_at(dir.path(), "a");
    let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
    run_sync(&mut a, &mut b);

    let item = |name: &str| PantryItemDoc {
        name: name.into(),
        presence: "have".into(),
        bought: None,
        tier: None,
        note: None,
    };

    // Enough edits on A to push B's pantry history across the 64-change
    // snapshot cadence when they arrive.
    for i in 0..70 {
        a.modify::<PantryDoc>(&DocId::Pantry(slug("home")), "seed", t0(), |p| {
            p.items.insert(format!("item-{i}"), item(&format!("item-{i}")));
        })
        .unwrap();
    }

    let mut pa = Peer::start(&a, true).unwrap();
    let mut pb = Peer::start(&b, false).unwrap();

    // The session is open; the desk edits the pantry behind its back.
    b.modify::<PantryDoc>(&DocId::Pantry(slug("home")), "desk", t0(), |p| {
        p.items.insert("mid-session".into(), item("mid-session"));
    })
    .unwrap();

    let mut msg = pa.initial_round(&a).unwrap();
    loop {
        let reply = pb.handle(&mut b, &msg).unwrap().expect("responder replies");
        match pa.handle(&mut a, &reply).unwrap() {
            Some(next) => msg = next,
            None => break,
        }
    }

    // The concurrent edit is still readable on B...
    let pantry: PantryDoc = b.get(&DocId::Pantry(slug("home"))).unwrap();
    assert!(
        pantry.items.contains_key("mid-session"),
        "a snapshot written from the session's stale doc hid the concurrent edit"
    );

    // ...and still syncable: the next session carries it to A.
    run_sync(&mut a, &mut b);
    let pantry_a: PantryDoc = a.get(&DocId::Pantry(slug("home"))).unwrap();
    assert!(pantry_a.items.contains_key("mid-session"));
    assert!(exports_equal(&a, &b));
}
