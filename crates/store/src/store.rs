//! The store proper: one SQLite file (`mise.db`) holding Automerge docs as
//! append-only change rows plus periodic snapshots, the append-only cook log,
//! and conversation threads. Beside it, `export/` — the read-only markdown
//! mirror, a git repo the store regenerates and commits after every change
//! batch.
//!
//! Nothing binary lives here: recon photos are conversation input that rides
//! a single exchange, never corpus state (see the M6 decisions in
//! docs/implementation.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use automerge::{AutoCommit, Change, ChangeHash};
use automerge::transaction::CommitOptions;
use autosurgeon::{Hydrate, Reconcile, hydrate, reconcile};
use jiff::Timestamp;
use mise_core::types::{CookKind, LocationView, LogEntry, Slug};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docid::DocId;
use crate::error::{Result, StoreError};
use crate::pages::{
    CorpusState, EquipmentDoc, FactsDoc, FridgeDoc, LocationDocs, PantryDoc, QueueDoc, RecipeDoc,
    ShoppingDoc, ShopsDoc, StateDoc, SteeringDoc, TechniqueDoc,
};
use crate::threads::{Role, ThreadId, ThreadMessage};

/// Snapshot cadence: a snapshot row is written every this-many changes, so
/// loading a doc replays at most this many changes past the latest snapshot.
const SNAPSHOT_EVERY: i64 = 64;

const SCHEMA: &str = "
CREATE TABLE docs (
  id   TEXT PRIMARY KEY,
  kind TEXT NOT NULL
) STRICT;
CREATE TABLE doc_changes (
  doc_id TEXT NOT NULL REFERENCES docs(id),
  seq    INTEGER NOT NULL,
  hash   TEXT NOT NULL,
  change BLOB NOT NULL,
  PRIMARY KEY (doc_id, seq)
) STRICT;
CREATE UNIQUE INDEX ux_doc_changes_hash ON doc_changes(doc_id, hash);
CREATE TABLE doc_snapshots (
  doc_id   TEXT NOT NULL REFERENCES docs(id),
  upto_seq INTEGER NOT NULL,
  snapshot BLOB NOT NULL,
  PRIMARY KEY (doc_id, upto_seq)
) STRICT;
CREATE TABLE cook_log (
  id       INTEGER PRIMARY KEY,
  uid      TEXT NOT NULL,
  date     TEXT NOT NULL,
  kind     TEXT NOT NULL,
  recipe   TEXT,
  title    TEXT NOT NULL,
  location TEXT NOT NULL,
  servings INTEGER NOT NULL,
  verdict  TEXT NOT NULL,
  tags     TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX ux_cook_log_uid ON cook_log(uid);
CREATE TABLE thread_messages (
  id      INTEGER PRIMARY KEY,
  uid     TEXT NOT NULL,
  thread  TEXT NOT NULL,
  role    TEXT NOT NULL,
  content TEXT NOT NULL,
  created TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX ux_thread_messages_uid ON thread_messages(uid);
CREATE INDEX ix_thread_messages_thread ON thread_messages(thread, created, uid);
-- Reserved, unused: photos are conversation input, not corpus state, so
-- nothing writes here. Kept because dropping it would need a schema bump for
-- no gain; no export promise covers it.
CREATE TABLE blobs (
  hash TEXT PRIMARY KEY,
  ext  TEXT NOT NULL
) STRICT;
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;
PRAGMA user_version = 5;
";

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Content-addressed identity prefix for a log row: append-only rows have no
/// CRDT, so cross-replica dedupe keys on content. The full uid is
/// `<hash16>-<n>` where `n` disambiguates genuinely repeated identical cooks.
///
/// This hash *is* the row's whole cross-replica identity: `ingest_log_row`
/// recomputes it and rejects the round on a mismatch. So the serialization can
/// never change silently — a different hash would reject every existing row on
/// every peer forever. The exhaustive destructure below makes a new field a
/// compile error (decide then: version the new form into the uid and accept
/// both prefixes forever), and `frozen_row_identity_never_moves` pins the
/// current output so any *representation* change (a `#[serde]` attr, field
/// order, a type) fails loudly rather than desyncing a live corpus.
fn log_content_hash(e: &LogEntry) -> String {
    use sha2::{Digest, Sha256};
    let LogEntry {
        date: _,
        kind: _,
        recipe: _,
        title: _,
        location: _,
        servings: _,
        verdict: _,
        tags: _,
    } = e;
    let canonical = serde_json::to_string(e).expect("log entries serialize");
    hex(&Sha256::digest(canonical.as_bytes()))[..16].to_string()
}

/// Same scheme for thread messages: content-hash prefix, occurrence suffix.
/// Frozen the same way as [`log_content_hash`] — the destructure and the
/// frozen-value test together forbid a silent identity change.
fn thread_content_hash(m: &ThreadMessage) -> String {
    use sha2::{Digest, Sha256};
    let ThreadMessage { thread: _, role: _, content: _, created: _ } = m;
    let canonical = serde_json::to_string(m).expect("thread messages serialize");
    hex(&Sha256::digest(canonical.as_bytes()))[..16].to_string()
}

/// Commit options carrying provenance and the caller-supplied clock.
/// Automerge change timestamps are unix seconds.
fn stamp(provenance: &str, at: Timestamp) -> CommitOptions {
    CommitOptions::default().with_message(provenance).with_time(at.as_second())
}

/// One entry in a document's history, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeInfo {
    /// Hex Automerge change hash — the handle for [`Store::revert`].
    pub hash: String,
    /// Provenance: which conversation or surface made the change.
    pub message: String,
    /// When, if the change was made by a clocked build (unix seconds; 0 in
    /// changes from before timestamps were threaded through).
    pub time: Option<Timestamp>,
}

/// Default source tiers for a fresh location; every one of these is an
/// ordinary edit away from being something else.
pub const DEFAULT_TIERS: &[(&str, &str)] = &[
    ("staples", "Staples — restock on sight"),
    ("shop", "Walkable shop"),
    ("butcher", "Butcher"),
    ("town", "Town"),
];

pub struct Store {
    conn: Connection,
    root: PathBuf,
    /// This store's random identity, minted once at first open and kept in
    /// `meta`. It scopes the occurrence index in log and thread uids, so
    /// each replica counts only its own repeats.
    replica: String,
}

/// Connection tuning, applied before anything else touches the database.
/// The design supports a `mise` CLI running beside the server on one file:
/// WAL lets readers run under a writer, and the busy timeout makes a second
/// writer wait instead of failing with an immediate "database is locked" —
/// which pairs with [`Store::transaction`] taking the write lock up front,
/// where the timeout can retry it (a deferred upgrade cannot).
fn tune(conn: &Connection) -> Result<()> {
    // journal_mode returns the resulting mode as a row, so pragma_update
    // would reject it.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

/// Mint the replica id if this store has none yet, and return it.
fn ensure_replica_id(conn: &Connection) -> Result<String> {
    let existing: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'replica_id'", [], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|e| StoreError::Io(std::io::Error::other(e)))?;
    let id = hex(&bytes);
    conn.execute("INSERT INTO meta (key, value) VALUES ('replica_id', ?1)", [&id])?;
    Ok(id)
}

impl Store {
    /// Initialize the on-disk layout with no documents at all. Callers other
    /// than tests almost always want [`Store::create`].
    pub fn create_bare(root: &Path) -> Result<Store> {
        let db = root.join("mise.db");
        if db.exists() {
            return Err(StoreError::AlreadyInitialized(root.to_path_buf()));
        }
        std::fs::create_dir_all(root)?;
        std::fs::create_dir_all(root.join("photos"))?;
        std::fs::create_dir_all(root.join("export"))?;
        let conn = Connection::open(&db)?;
        tune(&conn)?;
        conn.execute_batch(SCHEMA)?;
        let replica = ensure_replica_id(&conn)?;
        let store = Store { conn, root: root.to_path_buf(), replica };
        store.git(&["init", "-q"])?;
        Ok(store)
    }

    /// Initialize a fresh corpus at `root`: `mise.db`, `photos/`, and the
    /// `export/` git repo, with empty global pages and one location.
    pub fn create(root: &Path, location: &Slug, headcount: u32, at: Timestamp) -> Result<Store> {
        let mut store = Store::create_bare(root)?;
        let provenance = "init: empty corpus";
        store.create_doc(&DocId::State, &StateDoc::new(location.as_str(), headcount), provenance, at)?;
        store.create_doc(&DocId::Queue, &QueueDoc::empty(), provenance, at)?;
        store.create_doc(&DocId::Someday, &QueueDoc::empty(), provenance, at)?;
        store.create_doc(&DocId::Shopping, &ShoppingDoc::empty(), provenance, at)?;
        store.create_doc(&DocId::Steering, &SteeringDoc::empty(), provenance, at)?;
        store.create_doc(&DocId::Facts, &FactsDoc::empty(), provenance, at)?;
        store.create_location_docs(location, provenance, at)?;
        Ok(store)
    }

    pub fn open(root: &Path) -> Result<Store> {
        let db = root.join("mise.db");
        if !db.exists() {
            return Err(StoreError::NoCorpus(root.to_path_buf()));
        }
        let conn = Connection::open(&db)?;
        tune(&conn)?;
        migrate(&conn)?;
        let replica = ensure_replica_id(&conn)?;
        Ok(Store { conn, root: root.to_path_buf(), replica })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn export_dir(&self) -> PathBuf {
        self.root.join("export")
    }

    // ------------------------------------------------------------ docs --

    pub(crate) fn load_doc(&self, id: &DocId) -> Result<AutoCommit> {
        let key = id.to_string();
        let exists: Option<String> = self
            .conn
            .query_row("SELECT id FROM docs WHERE id = ?1", [&key], |r| r.get(0))
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::NotFound(key));
        }
        load_doc_rows(&self.conn, &key)
    }

    pub fn exists(&self, id: &DocId) -> Result<bool> {
        let found: Option<String> = self
            .conn
            .query_row("SELECT id FROM docs WHERE id = ?1", [id.to_string()], |r| r.get(0))
            .optional()?;
        Ok(found.is_some())
    }

    /// Doc ids of one kind, in id order.
    pub fn list(&self, kind: &str) -> Result<Vec<DocId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM docs WHERE kind = ?1 ORDER BY id")?;
        let ids = stmt
            .query_map([kind], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.iter().map(|s| DocId::parse(s)).collect()
    }

    pub fn get<T: Hydrate>(&self, id: &DocId) -> Result<T> {
        Ok(hydrate(&self.load_doc(id)?)?)
    }

    /// Persist one committed change from `doc`, snapshotting on cadence.
    fn persist_change(&mut self, key: &str, doc: &mut AutoCommit) -> Result<()> {
        let change = doc
            .get_last_local_change()
            .expect("commit reported a change")
            .clone();
        self.transaction(|tx| tx.append_changes(key, &[change]))?;
        Ok(())
    }

    /// Run one atomic write unit; committed only if `f` returns Ok. Every
    /// path that writes more than one row goes through here, so a kill, a
    /// full disk, or SQLITE_BUSY between statements cannot leave a doc row
    /// without its changes, or half a sync round's sibling docs.
    pub(crate) fn transaction<R>(
        &mut self,
        f: impl FnOnce(&StoreTx<'_>) -> Result<R>,
    ) -> Result<R> {
        // Immediate: take the write lock at BEGIN, where the busy timeout
        // can wait for it. A deferred transaction that reads before writing
        // needs a lock upgrade, and an upgrade that finds another writer
        // fails with SQLITE_BUSY no matter the timeout.
        let stx = StoreTx {
            tx: self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?,
        };
        let out = f(&stx)?;
        stx.tx.commit()?;
        Ok(out)
    }

    /// Every doc id in the store, in id order.
    pub(crate) fn all_doc_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM docs ORDER BY id")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Create a new doc from an initial value. `provenance` lands in the
    /// Automerge change message (which conversation or surface did this)
    /// and `at` as its timestamp — the store never reads a clock.
    pub fn create_doc<T: Reconcile>(
        &mut self,
        id: &DocId,
        value: &T,
        provenance: &str,
        at: Timestamp,
    ) -> Result<()> {
        let key = id.to_string();
        if self.exists(id)? {
            return Err(StoreError::Exists(key));
        }
        let mut doc = AutoCommit::new();
        reconcile(&mut doc, value)?;
        let committed = doc.commit_with(stamp(provenance, at));
        // One transaction: a doc row without its first change is a doc no
        // read can hydrate, and `Exists` blocks the retry that would fix it.
        self.transaction(|tx| {
            tx.insert_doc_row(id)?;
            if committed.is_some() {
                let change = doc
                    .get_last_local_change()
                    .expect("commit reported a change")
                    .clone();
                tx.append_changes(&key, &[change])?;
            }
            Ok(())
        })
    }

    /// Hydrate, mutate, reconcile, persist. Returns the new value. A no-op
    /// mutation writes nothing.
    ///
    /// The stamp is applied *after* the closure, so no mutation — nor a revert
    /// restoring a historical value — can persist new-shape bytes under an old
    /// `schema_version`. Stamping to the current version is idempotent, so a
    /// closure that changed nothing still reconciles to no change and writes
    /// nothing.
    pub fn modify<T: Hydrate + Reconcile + crate::pages::Stamped>(
        &mut self,
        id: &DocId,
        provenance: &str,
        at: Timestamp,
        f: impl FnOnce(&mut T),
    ) -> Result<T> {
        let mut doc = self.load_doc(id)?;
        let mut value: T = hydrate(&doc)?;
        f(&mut value);
        value.stamp();
        reconcile(&mut doc, &value)?;
        let committed = doc.commit_with(stamp(provenance, at));
        if committed.is_some() {
            self.persist_change(&id.to_string(), &mut doc)?;
        }
        Ok(value)
    }

    /// Replace a prose body (the root-level `body` text of a recipe or
    /// technique) by grapheme-level diff, splicing directly through
    /// Automerge in its native character units.
    ///
    /// This exists because autosurgeon 0.8's `Text::update` advances splice
    /// positions in *bytes* while Automerge indexes text by unicode
    /// scalars; any non-ASCII body walks the indices off the end of the
    /// text. Production body edits must come through here — never
    /// `Text::update`/`Text::splice`.
    pub fn update_body(
        &mut self,
        id: &DocId,
        new_body: &str,
        provenance: &str,
        at: Timestamp,
    ) -> Result<()> {
        use automerge::transaction::Transactable;
        use automerge::{ObjType, ReadDoc, Value};

        let mut doc = self.load_doc(id)?;
        let Some((Value::Object(ObjType::Text), obj)) = doc.get(automerge::ROOT, "body")?
        else {
            return Err(StoreError::Invalid(format!("{id} has no prose body")));
        };
        let old = doc.text(&obj)?;
        let mut idx = 0usize;
        for change in similar::TextDiff::from_graphemes(old.as_str(), new_body).iter_all_changes()
        {
            let chunk = change.value();
            let chars = chunk.chars().count();
            match change.tag() {
                similar::ChangeTag::Delete => {
                    doc.splice_text(&obj, idx, isize::try_from(chars).expect("chunk fits"), "")?;
                }
                similar::ChangeTag::Insert => {
                    doc.splice_text(&obj, idx, 0, chunk)?;
                    idx += chars;
                }
                similar::ChangeTag::Equal => idx += chars,
            }
        }
        let committed = doc.commit_with(stamp(provenance, at));
        if committed.is_some() {
            self.persist_change(&id.to_string(), &mut doc)?;
        }
        Ok(())
    }

    fn create_location_docs(&mut self, location: &Slug, provenance: &str, at: Timestamp) -> Result<()> {
        let docs = LocationDocs::empty_with_tiers(DEFAULT_TIERS);
        self.create_doc(&DocId::Pantry(location.clone()), &docs.pantry, provenance, at)?;
        self.create_doc(&DocId::Equipment(location.clone()), &docs.equipment, provenance, at)?;
        self.create_doc(&DocId::Shops(location.clone()), &docs.shops, provenance, at)?;
        self.create_doc(&DocId::Fridge(location.clone()), &docs.fridge, provenance, at)?;
        Ok(())
    }

    /// Register a new location: its four docs plus the state-page entry.
    pub fn add_location(
        &mut self,
        location: &Slug,
        headcount: u32,
        provenance: &str,
        at: Timestamp,
    ) -> Result<()> {
        if self.exists(&DocId::Pantry(location.clone()))? {
            return Err(StoreError::Exists(format!("location {location}")));
        }
        self.create_location_docs(location, provenance, at)?;
        self.modify::<StateDoc>(&DocId::State, provenance, at, |s| {
            s.locations.insert(
                location.as_str().to_string(),
                crate::pages::LocationMeta { headcount },
            );
        })?;
        Ok(())
    }

    // ------------------------------------------------------------- log --

    /// Append a cook. The row's uid is its content hash plus this replica's
    /// id and occurrence index, so a cook is identified by who recorded it:
    /// the row reaches every device exactly once, and repeats — even
    /// partitioned ones — stay distinct rows.
    ///
    /// A first cook promotes a draft recipe to active — that rule lives here
    /// so no caller can log a cook and forget it. Promotion is a doc change
    /// (stamped with `provenance`/`at`) that syncs like any other; the log
    /// row itself is clockless. The sync insert path does not promote — the
    /// origin device already did, and its doc change is on the way.
    pub fn append_log(&mut self, e: &LogEntry, provenance: &str, at: Timestamp) -> Result<String> {
        // The occurrence index counts only this replica's own repeats — the
        // uid space is partitioned by replica id, so two devices logging the
        // same dish while apart cannot mint colliding uids and collapse
        // genuinely distinct cooks on merge.
        let scope = format!("{}-{}", log_content_hash(e), self.replica);
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cook_log WHERE uid LIKE ?1 || '-%'",
            [&scope],
            |r| r.get(0),
        )?;
        let uid = format!("{scope}-{n}");
        if !self.insert_log_row(&uid, e)? {
            return Err(StoreError::Corrupt(format!("log uid {uid} already taken")));
        }
        if let Some(slug) = &e.recipe {
            let id = DocId::Recipe(slug.clone());
            if self.exists(&id)? {
                self.modify::<crate::pages::RecipeDoc>(&id, provenance, at, |r| {
                    if r.status == mise_core::types::RecipeStatus::Draft {
                        r.status = mise_core::types::RecipeStatus::Active;
                    }
                })?;
            }
        }
        Ok(uid)
    }

    /// A fresh collection-item id — `<prefix>-<replica>-<seq>` — from a
    /// monotonic per-store counter. Shopping items and fridge portions are
    /// CRDT map keys, so their ids are their identity: positional ids
    /// (`s1`) collide across replicas, where the merge resolves both puts
    /// to one winner and the other item silently vanishes; and lowest-free
    /// reuse lets a stale remove delete a stranger. The counter never
    /// reuses, and legacy positional keys stay inert — never reused, never
    /// renumbered.
    pub fn mint_id(&mut self, prefix: &str) -> Result<String> {
        let seq: i64 = self.conn.query_row(
            "INSERT INTO meta (key, value) VALUES ('id_seq', '1')
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + 1
             RETURNING CAST(value AS INTEGER)",
            [],
            |r| r.get(0),
        )?;
        Ok(format!("{prefix}-{}-{seq}", self.replica))
    }

    /// Idempotent insert of a log row with a known uid.
    pub(crate) fn insert_log_row(&mut self, uid: &str, e: &LogEntry) -> Result<bool> {
        self.transaction(|tx| tx.insert_log_row(uid, e))
    }

    pub(crate) fn log_uids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT uid FROM cook_log ORDER BY uid")?;
        let uids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(uids)
    }

    /// All log rows with their uids, in (date, uid) order. One statement —
    /// the uid and its entry come from the same row of the same read
    /// snapshot. This used to be two independent SELECTs zipped together,
    /// and in WAL mode a concurrent writer (the CLI beside the server)
    /// committing between them misaligned every pair after the insertion
    /// point; `zip` then truncated the mismatch silently and the wrong
    /// pairs went onto the sync wire. `thread_rows` has the same shape.
    pub(crate) fn log_rows(&self) -> Result<Vec<(String, LogEntry)>> {
        let mut stmt = self.conn.prepare(
            "SELECT uid, date, kind, recipe, title, location, servings, verdict, tags
             FROM cook_log ORDER BY date, uid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, u32>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(uid, date, kind, recipe, title, location, servings, verdict, tags)| {
                let corrupt = |m: String| StoreError::Corrupt(format!("log row: {m}"));
                Ok((
                    uid,
                    LogEntry {
                        date: date.parse().map_err(|e| corrupt(format!("bad date: {e}")))?,
                        kind: kind.parse::<CookKind>().map_err(corrupt)?,
                        recipe: recipe
                            .map(|s| Slug::new(s).map_err(|e| corrupt(e.to_string())))
                            .transpose()?,
                        title,
                        location,
                        servings,
                        verdict,
                        tags: serde_json::from_str(&tags)?,
                    },
                ))
            })
            .collect()
    }

    /// The whole log, ordered by (date, uid) — deterministic across
    /// replicas. Derived from `log_rows` so the two orderings cannot
    /// diverge: there is only one query.
    pub fn log_entries(&self) -> Result<Vec<LogEntry>> {
        Ok(self.log_rows()?.into_iter().map(|(_, e)| e).collect())
    }

    // --------------------------------------------------------- history --

    /// A document's full change history, oldest first. This is the "recent
    /// changes" feed: what, when, from which conversation.
    pub fn history(&self, id: &DocId) -> Result<Vec<ChangeInfo>> {
        let mut doc = self.load_doc(id)?;
        Ok(doc
            .get_changes(&[])
            .into_iter()
            .map(|c| ChangeInfo {
                hash: hex(&c.hash().0),
                message: c.message().cloned().unwrap_or_default(),
                time: (c.timestamp() != 0).then(|| Timestamp::from_second(c.timestamp()))
                    .transpose()
                    .ok()
                    .flatten(),
            })
            .collect())
    }

    /// Restore a page to its state as of `hash` (a change from
    /// [`Store::history`]), recorded as a new forward change — history only
    /// ever grows, and the revert itself is visible and revertible.
    pub fn revert(&mut self, id: &DocId, hash: &str, provenance: &str, at: Timestamp) -> Result<()> {
        let mut doc = self.load_doc(id)?;
        let target: ChangeHash = hash
            .parse()
            .map_err(|_| StoreError::Invalid(format!("not a change hash: {hash:?}")))?;
        if doc.get_change_by_hash(&target).is_none() {
            return Err(StoreError::NotFound(format!("change {hash} in {id}")));
        }
        let old = doc.fork_at(&[target])?;

        match id {
            DocId::State => self.revert_plain::<StateDoc>(id, &old, provenance, at),
            DocId::Queue | DocId::Someday => {
                self.revert_plain::<QueueDoc>(id, &old, provenance, at)
            }
            DocId::Shopping => self.revert_plain::<ShoppingDoc>(id, &old, provenance, at),
            DocId::Steering => self.revert_plain::<SteeringDoc>(id, &old, provenance, at),
            DocId::Facts => self.revert_plain::<FactsDoc>(id, &old, provenance, at),
            DocId::Pantry(_) => self.revert_plain::<PantryDoc>(id, &old, provenance, at),
            DocId::Equipment(_) => self.revert_plain::<EquipmentDoc>(id, &old, provenance, at),
            DocId::Shops(_) => self.revert_plain::<ShopsDoc>(id, &old, provenance, at),
            DocId::Fridge(_) => self.revert_plain::<FridgeDoc>(id, &old, provenance, at),
            // Prose pages: scalar fields through reconcile, the body through
            // the char-safe splice path (a hydrated Text from the historical
            // fork cannot reconcile onto the current doc).
            //
            // Both arms *destructure* the historical value rather than
            // listing the fields they care about, so a new field on either
            // doc is a compile error here instead of a field that silently
            // stops being revertible. `source` was exactly that: absent from
            // the old hand-written list, so a wrong source URL could not be
            // undone from the history UI at all.
            DocId::Recipe(_) => {
                let value: RecipeDoc = hydrate(&old)?;
                // `schema_version` is bound but not restored — `modify` stamps
                // the current version, so a revert never carries an old stamp
                // forward. It stays in the destructure so a new field is still
                // a compile error here (see `source`, which was silently lost).
                let RecipeDoc {
                    schema_version: _,
                    title,
                    servings,
                    effort,
                    lead,
                    tags,
                    equipment,
                    ingredients,
                    source,
                    status,
                    body,
                } = value;
                let old_body = body.as_str().to_string();
                self.modify::<RecipeDoc>(id, provenance, at, |r| {
                    r.title = title;
                    r.servings = servings;
                    r.effort = effort;
                    r.lead = lead;
                    r.tags = tags;
                    r.equipment = equipment;
                    r.ingredients = ingredients;
                    r.source = source;
                    r.status = status;
                    // `body` is spliced below, not reconciled.
                })?;
                self.update_body(id, &old_body, provenance, at)
            }
            DocId::Technique(_) => {
                let value: TechniqueDoc = hydrate(&old)?;
                let TechniqueDoc { schema_version: _, title, tags, body } = value;
                let old_body = body.as_str().to_string();
                self.modify::<TechniqueDoc>(id, provenance, at, |t| {
                    t.title = title;
                    t.tags = tags;
                })?;
                self.update_body(id, &old_body, provenance, at)
            }
        }
    }

    fn revert_plain<T: Hydrate + Reconcile + crate::pages::Stamped>(
        &mut self,
        id: &DocId,
        old: &AutoCommit,
        provenance: &str,
        at: Timestamp,
    ) -> Result<()> {
        let value: T = hydrate(old)?;
        self.modify::<T>(id, provenance, at, |v| *v = value)?;
        Ok(())
    }

    // --------------------------------------------------------- threads --

    /// Append one turn to a thread. Content is normalized (LF line endings,
    /// trimmed); an empty message is refused. Returns the row uid — content
    /// hash plus occurrence index, exactly like the cook log.
    pub fn append_thread_message(
        &mut self,
        thread: &ThreadId,
        role: Role,
        content: &str,
        created: jiff::civil::DateTime,
    ) -> Result<String> {
        let msg = ThreadMessage {
            thread: thread.clone(),
            role,
            content: content.to_string(),
            created,
        }
        .normalized()
        .ok_or_else(|| StoreError::Invalid("empty thread message".into()))?;
        let scope = format!("{}-{}", thread_content_hash(&msg), self.replica);
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM thread_messages WHERE uid LIKE ?1 || '-%'",
            [&scope],
            |r| r.get(0),
        )?;
        let uid = format!("{scope}-{n}");
        if !self.insert_thread_row(&uid, &msg)? {
            return Err(StoreError::Corrupt(format!("thread uid {uid} already taken")));
        }
        Ok(uid)
    }

    /// Idempotent insert of a thread row with a known uid.
    pub(crate) fn insert_thread_row(&mut self, uid: &str, m: &ThreadMessage) -> Result<bool> {
        self.transaction(|tx| tx.insert_thread_row(uid, m))
    }

    pub(crate) fn thread_uids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT uid FROM thread_messages ORDER BY uid")?;
        let uids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(uids)
    }

    fn thread_row(
        row: (String, String, String, String),
    ) -> Result<ThreadMessage> {
        let (thread, role, content, created) = row;
        let corrupt = |m: String| StoreError::Corrupt(format!("thread row: {m}"));
        Ok(ThreadMessage {
            thread: ThreadId::parse(&thread).map_err(|e| corrupt(e.to_string()))?,
            role: role.parse().map_err(corrupt)?,
            content,
            created: created.parse().map_err(|e| corrupt(format!("bad datetime: {e}")))?,
        })
    }

    /// All thread rows with their uids, ordered (thread, created, uid).
    pub(crate) fn thread_rows(&self) -> Result<Vec<(String, ThreadMessage)>> {
        let mut stmt = self.conn.prepare(
            "SELECT uid, thread, role, content, created
             FROM thread_messages ORDER BY thread, created, uid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?),
                ))
            })?
            .collect::<rusqlite::Result<Vec<(String, _)>>>()?;
        rows.into_iter()
            .map(|(uid, raw)| Ok((uid, Store::thread_row(raw)?)))
            .collect()
    }

    /// The latest stamp on one thread — what a new message must sort
    /// after. Textual MAX matches chronological order because the stamps
    /// serialize in ISO form.
    pub fn last_thread_stamp(
        &self,
        thread: &ThreadId,
    ) -> Result<Option<jiff::civil::DateTime>> {
        let created: Option<String> = self.conn.query_row(
            "SELECT MAX(created) FROM thread_messages WHERE thread = ?1",
            [thread.to_string()],
            |r| r.get(0),
        )?;
        created
            .map(|c| {
                c.parse().map_err(|e| {
                    StoreError::Corrupt(format!("thread row: bad datetime: {e}"))
                })
            })
            .transpose()
    }

    /// One thread's messages in (created, uid) order — deterministic across
    /// replicas, same argument as the cook log.
    pub fn thread_messages(&self, thread: &ThreadId) -> Result<Vec<ThreadMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT thread, role, content, created FROM thread_messages
             WHERE thread = ?1 ORDER BY created, uid",
        )?;
        let rows = stmt
            .query_map([thread.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(Store::thread_row).collect()
    }

    /// Every non-empty thread, keyed by thread id string, messages in
    /// (created, uid) order.
    pub fn threads(&self) -> Result<BTreeMap<String, Vec<ThreadMessage>>> {
        let mut out: BTreeMap<String, Vec<ThreadMessage>> = BTreeMap::new();
        for (_, msg) in self.thread_rows()? {
            out.entry(msg.thread.to_string()).or_default().push(msg);
        }
        Ok(out)
    }

    // ---------------------------------------------------------- corpus --

    /// `get`, with an absent doc supplied by `empty` — for location
    /// siblings, where a gap degrades instead of erasing the location.
    /// Only `NotFound` degrades; a doc that exists but will not hydrate is
    /// still a loud error.
    fn get_or<T: Hydrate>(&self, id: &DocId, empty: impl FnOnce() -> T) -> Result<T> {
        match self.get(id) {
            Err(StoreError::NotFound(_)) => Ok(empty()),
            other => other,
        }
    }

    /// Hydrate everything: the render layer's input.
    pub fn corpus(&self) -> Result<CorpusState> {
        // Locations are the union of all four kinds, and a missing sibling
        // hydrates as empty. Partial sets are reachable — a kill between
        // the four per-doc creates, an interrupted first sync — and one gap
        // must not erase the other three docs from every read and from the
        // export, whose stale-file pass would delete them as orphans.
        let mut slugs: BTreeMap<String, Slug> = BTreeMap::new();
        for kind in ["pantry", "equipment", "shops", "fridge"] {
            for id in self.list(kind)? {
                let (DocId::Pantry(l)
                | DocId::Equipment(l)
                | DocId::Shops(l)
                | DocId::Fridge(l)) = id
                else {
                    unreachable!()
                };
                slugs.insert(l.as_str().to_string(), l);
            }
        }
        let mut locations = BTreeMap::new();
        for (name, loc) in slugs {
            let docs = LocationDocs {
                pantry: self.get_or(&DocId::Pantry(loc.clone()), PantryDoc::empty)?,
                equipment: self.get_or(&DocId::Equipment(loc.clone()), EquipmentDoc::empty)?,
                shops: self.get_or(&DocId::Shops(loc.clone()), || ShopsDoc::new(&[]))?,
                fridge: self.get_or(&DocId::Fridge(loc.clone()), FridgeDoc::empty)?,
            };
            locations.insert(name, docs);
        }
        let mut recipes = BTreeMap::new();
        for id in self.list("recipe")? {
            let DocId::Recipe(slug) = id else { unreachable!() };
            recipes.insert(slug.as_str().to_string(), self.get(&DocId::Recipe(slug.clone()))?);
        }
        let mut techniques = BTreeMap::new();
        for id in self.list("technique")? {
            let DocId::Technique(slug) = id else { unreachable!() };
            techniques.insert(slug.as_str().to_string(), self.get(&DocId::Technique(slug.clone()))?);
        }
        Ok(CorpusState {
            state: self.get(&DocId::State)?,
            queue: self.get(&DocId::Queue)?,
            someday: self.get(&DocId::Someday)?,
            shopping: self.get(&DocId::Shopping)?,
            steering: self.get(&DocId::Steering)?,
            facts: self.get(&DocId::Facts)?,
            locations,
            recipes,
            techniques,
            log: self.log_entries()?,
            threads: self.threads()?,
        })
    }

    /// Render the one page a doc exports to, from just that doc — the
    /// assistant's context assembly needs three or four pages, and rendering
    /// the whole corpus for them rendered every thread transcript in full
    /// while holding the store mutex. Agreement with the full export is a
    /// property test: same doc, same bytes as `render(&corpus())`.
    ///
    /// The four location kinds hydrate empty when the doc is missing, like
    /// `corpus()` — partial sibling sets are reachable and render as empty
    /// pages there too. Any other missing doc renders as an empty string (a
    /// page thread can outlive its deleted page).
    pub fn render_page(&self, id: &DocId) -> Result<String> {
        use crate::render as r;
        let page = match id {
            DocId::Pantry(l) => r::pantry_page(l.as_str(), &self.get_or(id, PantryDoc::empty)?),
            DocId::Equipment(l) => {
                r::equipment_page(l.as_str(), &self.get_or(id, EquipmentDoc::empty)?)
            }
            DocId::Shops(l) => r::shops_page(l.as_str(), &self.get_or(id, || ShopsDoc::new(&[]))?),
            DocId::Fridge(l) => r::fridge_page(l.as_str(), &self.get_or(id, FridgeDoc::empty)?),
            _ => {
                if !self.exists(id)? {
                    return Ok(String::new());
                }
                match id {
                    DocId::State => r::state_page(&self.get(id)?),
                    DocId::Queue => r::queue_page("Queue", &self.get(id)?),
                    DocId::Someday => r::queue_page("Someday", &self.get(id)?),
                    DocId::Shopping => r::shopping_page(&self.get(id)?),
                    DocId::Steering => {
                        let doc: SteeringDoc = self.get(id)?;
                        r::kv_page("Steering", "note", &doc.schema_version, &doc.entries)
                    }
                    DocId::Facts => {
                        let doc: FactsDoc = self.get(id)?;
                        r::kv_page("Facts", "fact", &doc.schema_version, &doc.facts)
                    }
                    DocId::Recipe(_) => r::recipe_page(&self.get(id)?),
                    DocId::Technique(_) => r::technique_page(&self.get(id)?),
                    DocId::Pantry(_)
                    | DocId::Equipment(_)
                    | DocId::Shops(_)
                    | DocId::Fridge(_) => unreachable!("handled above"),
                }
            }
        };
        Ok(page)
    }

    /// The plain view of one location, for readiness and coverage.
    pub fn location_view(&self, location: &Slug) -> Result<LocationView> {
        let state: StateDoc = self.get(&DocId::State)?;
        let meta = state
            .locations
            .get(location.as_str())
            .ok_or_else(|| StoreError::NotFound(format!("location {location}")))?;
        // A partial sibling set is reachable — a kill between the four
        // per-doc creates in `add_location`, an interrupted first sync — so a
        // missing sibling degrades to empty exactly as `corpus()` and
        // `render_page` do, rather than 500ing readiness, `/api/queue` and
        // every chat turn while the export renders the same location fine.
        let docs = LocationDocs {
            pantry: self.get_or(&DocId::Pantry(location.clone()), PantryDoc::empty)?,
            equipment: self.get_or(&DocId::Equipment(location.clone()), EquipmentDoc::empty)?,
            shops: self.get_or(&DocId::Shops(location.clone()), || ShopsDoc::new(&[]))?,
            fridge: self.get_or(&DocId::Fridge(location.clone()), FridgeDoc::empty)?,
        };
        Ok(docs.to_view(location.as_str(), meta))
    }

    /// The active location's plain view.
    pub fn active_view(&self) -> Result<(Slug, LocationView)> {
        let state: StateDoc = self.get(&DocId::State)?;
        let slug = Slug::new(state.active_location.as_str())
            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
        Ok((slug.clone(), self.location_view(&slug)?))
    }

    // ---------------------------------------------------------- export --

    /// Regenerate the markdown export and commit it. One commit per change
    /// batch; `message` carries provenance. No-ops when nothing changed.
    pub fn export(&mut self, message: &str) -> Result<()> {
        let files = crate::render::render(&self.corpus()?);
        let dir = self.export_dir();

        // The export is derived state, promised deletable and regenerable at
        // any time — so heal the directory and its repo before writing.
        // Without this, the first export after a deletion fails *after* the
        // SQLite mutation committed, permanently, and the natural retry
        // duplicates log rows.
        std::fs::create_dir_all(&dir)?;
        if !dir.join(".git").exists() {
            self.git(&["init", "-q"])?;
        }

        // Write the rendered tree, then remove any stale files.
        for (rel, content) in &files {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, content)?;
        }
        let mut existing = Vec::new();
        collect_files(&dir, &dir, &mut existing)?;
        for rel in existing {
            if !files.contains_key(&rel) {
                std::fs::remove_file(dir.join(&rel))?;
            }
        }
        remove_empty_dirs(&dir, &dir)?;

        let status = self.git(&["status", "--porcelain"])?;
        if !status.is_empty() {
            self.git(&["add", "-A"])?;
            self.git(&["commit", "-q", "-m", message])?;
        }
        Ok(())
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let dir = self.export_dir();
        let base = [
            "-C",
            dir.to_str().expect("export path is valid UTF-8"),
            "-c",
            "user.name=mise",
            "-c",
            "user.email=mise@localhost",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ];
        let output = Command::new("git").args(base).args(args).output()?;
        if !output.status.success() {
            return Err(StoreError::Git {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

/// Bring an existing database up to the current schema. Fresh databases are
/// created at the current version by `SCHEMA` directly.
/// The doc as its rows say it is: latest snapshot plus replay. Takes the
/// connection (or a live transaction, which derefs to one) so snapshot
/// writers inside a transaction rebuild from what they just wrote.
fn load_doc_rows(conn: &Connection, key: &str) -> Result<AutoCommit> {
    let snapshot: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT upto_seq, snapshot FROM doc_snapshots
             WHERE doc_id = ?1 ORDER BY upto_seq DESC LIMIT 1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (from_seq, mut doc) = match snapshot {
        Some((upto, bytes)) => (upto, AutoCommit::load(&bytes)?),
        None => (0, AutoCommit::new()),
    };
    let mut stmt = conn.prepare(
        "SELECT change FROM doc_changes WHERE doc_id = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let changes = stmt
        .query_map(params![key, from_seq], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let changes = changes
        .into_iter()
        .map(Change::from_bytes)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    doc.apply_changes(changes)?;
    Ok(doc)
}

/// One atomic write unit: a doc creation, or everything a sync round
/// persists. Constructed only by [`Store::transaction`]; nothing inside is
/// visible to readers — or survives a failure — until it commits whole.
pub(crate) struct StoreTx<'a> {
    tx: rusqlite::Transaction<'a>,
}

impl StoreTx<'_> {
    /// The doc row for a brand-new doc; fails if it already exists.
    pub(crate) fn insert_doc_row(&self, id: &DocId) -> Result<()> {
        self.tx.execute(
            "INSERT INTO docs (id, kind) VALUES (?1, ?2)",
            params![id.to_string(), id.kind()],
        )?;
        Ok(())
    }

    /// Make sure a doc row exists (sync may introduce docs we've never seen).
    pub(crate) fn ensure_doc_row(&self, id: &DocId) -> Result<()> {
        self.tx.execute(
            "INSERT OR IGNORE INTO docs (id, kind) VALUES (?1, ?2)",
            params![id.to_string(), id.kind()],
        )?;
        Ok(())
    }

    /// Append changes to a doc's history, deduplicating by change hash —
    /// sync can deliver a change along more than one path. Returns how many
    /// rows were actually new.
    pub(crate) fn append_changes(&self, key: &str, changes: &[Change]) -> Result<usize> {
        let mut seq: i64 = self.tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM doc_changes WHERE doc_id = ?1",
            [key],
            |r| r.get::<_, i64>(0),
        )?;
        let mut inserted = 0;
        for change in changes {
            let hash = hex(&change.hash().0);
            let known: Option<i64> = self
                .tx
                .query_row(
                    "SELECT seq FROM doc_changes WHERE doc_id = ?1 AND hash = ?2",
                    params![key, hash],
                    |r| r.get(0),
                )
                .optional()?;
            if known.is_some() {
                continue;
            }
            seq += 1;
            self.tx.execute(
                "INSERT INTO doc_changes (doc_id, seq, hash, change) VALUES (?1, ?2, ?3, ?4)",
                params![key, seq, hash, change.raw_bytes()],
            )?;
            inserted += 1;
            if seq % SNAPSHOT_EVERY == 0 {
                // Rebuilt from the rows, never saved from a caller's doc: a
                // sync session's in-memory doc predates concurrent writers
                // whose rows are already below this boundary, and a snapshot
                // missing them hides their changes from every later load.
                let mut doc = load_doc_rows(&self.tx, key)?;
                self.tx.execute(
                    "INSERT INTO doc_snapshots (doc_id, upto_seq, snapshot) VALUES (?1, ?2, ?3)",
                    params![key, seq, doc.save()],
                )?;
            }
        }
        Ok(inserted)
    }

    /// Idempotent insert of a log row with a known uid.
    pub(crate) fn insert_log_row(&self, uid: &str, e: &LogEntry) -> Result<bool> {
        let inserted = self.tx.execute(
            "INSERT OR IGNORE INTO cook_log
               (uid, date, kind, recipe, title, location, servings, verdict, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                uid,
                e.date.to_string(),
                e.kind.to_string(),
                e.recipe.as_ref().map(|s| s.as_str().to_string()),
                e.title,
                e.location,
                e.servings,
                e.verdict,
                serde_json::to_string(&e.tags)?,
            ],
        )?;
        Ok(inserted > 0)
    }

    /// Idempotent insert of a thread row with a known uid.
    pub(crate) fn insert_thread_row(&self, uid: &str, m: &ThreadMessage) -> Result<bool> {
        let inserted = self.tx.execute(
            "INSERT OR IGNORE INTO thread_messages (uid, thread, role, content, created)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                uid,
                m.thread.to_string(),
                m.role.to_string(),
                m.content,
                m.created.to_string(),
            ],
        )?;
        Ok(inserted > 0)
    }

    /// Sync ingress for a log row. The uid is the row's entire cross-replica
    /// identity and it arrived off the wire, so rebind it to the content:
    /// it must be `sha256(entry)[..16]-<nonempty suffix>` (the suffix form
    /// changed over time; the content hash is the invariant). A mismatch
    /// rejects the round — `INSERT OR IGNORE` would otherwise let a forged
    /// uid shadow the genuine row forever.
    pub(crate) fn ingest_log_row(&self, uid: &str, e: &LogEntry) -> Result<bool> {
        let prefix = format!("{}-", log_content_hash(e));
        if uid.strip_prefix(&prefix).is_none_or(|suffix| suffix.is_empty()) {
            return Err(StoreError::Corrupt(format!(
                "log row uid {uid:?} does not match its content"
            )));
        }
        self.insert_log_row(uid, e)
    }

    /// Sync ingress for a thread row: normalize exactly as local append
    /// does, then verify the uid against the normalized form — otherwise
    /// identical turns hash to different uids across replicas and raw CRLF
    /// reaches the renderer.
    pub(crate) fn ingest_thread_row(&self, uid: &str, m: &ThreadMessage) -> Result<bool> {
        let m = m.clone().normalized().ok_or_else(|| {
            StoreError::Corrupt(format!("thread row {uid:?} is empty after normalization"))
        })?;
        let prefix = format!("{}-", thread_content_hash(&m));
        if uid.strip_prefix(&prefix).is_none_or(|suffix| suffix.is_empty()) {
            return Err(StoreError::Corrupt(format!(
                "thread row uid {uid:?} does not match its content"
            )));
        }
        self.insert_thread_row(uid, &m)
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    match version {
        5 => Ok(()),
        4 => migrate_v4_to_v5(conn),
        3 => {
            migrate_v3_to_v4(conn)?;
            migrate_v4_to_v5(conn)
        }
        2 => {
            migrate_v2_to_v3(conn)?;
            migrate_v3_to_v4(conn)?;
            migrate_v4_to_v5(conn)
        }
        1 => {
            migrate_v1_to_v2(conn)?;
            migrate_v2_to_v3(conn)?;
            migrate_v3_to_v4(conn)?;
            migrate_v4_to_v5(conn)
        }
        other => Err(StoreError::Corrupt(format!(
            "unsupported schema version {other} (this build understands 1..=5)"
        ))),
    }
}

/// v4 → v5: a `meta` table for per-store facts, first of them the replica
/// id that scopes new log/thread uids. Existing rows keep their two-part
/// uids forever — sync verifies rows by content-hash prefix, not format.
fn migrate_v4_to_v5(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE meta (
           key   TEXT PRIMARY KEY,
           value TEXT NOT NULL
         ) STRICT;
         PRAGMA user_version = 5;
         COMMIT;",
    )?;
    Ok(())
}

/// v3 → v4: every snapshot is dropped and rebuilt from a full replay of
/// `doc_changes`. Builds before the snapshot-rebuild fix could save a sync
/// session's stale doc as the cadence snapshot, hiding concurrent writers'
/// changes from every load — but the change rows themselves were always
/// intact, so a pure replay recovers everything a stale snapshot hid. No
/// DDL; this is a data repair riding the version ladder so it runs exactly
/// once.
fn migrate_v3_to_v4(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM doc_snapshots", [])?;
    let docs: Vec<(String, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT doc_id, MAX(seq) FROM doc_changes GROUP BY doc_id HAVING MAX(seq) >= ?1",
        )?;
        stmt.query_map([SNAPSHOT_EVERY], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (key, max_seq) in docs {
        let mut doc = load_doc_rows(&tx, &key)?;
        tx.execute(
            "INSERT INTO doc_snapshots (doc_id, upto_seq, snapshot) VALUES (?1, ?2, ?3)",
            params![key, max_seq, doc.save()],
        )?;
    }
    tx.pragma_update(None, "user_version", 4)?;
    tx.commit()?;
    Ok(())
}

/// v2 → v3: thread rows gain the same content-hash uid identity as the log,
/// and the separate `threads` table folds into a `thread` column. No code
/// path in any v2 build ever inserted a thread row — the tables existed but
/// were unreachable — so this is a drop-and-recreate, not a data migration.
fn migrate_v2_to_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         DROP TABLE thread_messages;
         DROP TABLE threads;
         CREATE TABLE thread_messages (
           id      INTEGER PRIMARY KEY,
           uid     TEXT NOT NULL,
           thread  TEXT NOT NULL,
           role    TEXT NOT NULL,
           content TEXT NOT NULL,
           created TEXT NOT NULL
         ) STRICT;
         CREATE UNIQUE INDEX ux_thread_messages_uid ON thread_messages(uid);
         CREATE INDEX ix_thread_messages_thread ON thread_messages(thread, created, uid);
         PRAGMA user_version = 3;
         COMMIT;",
    )?;
    Ok(())
}

/// v1 → v2: change rows gain a content-hash column (sync dedupe), log rows
/// gain a content-derived uid (append-merge identity). Both are derivable
/// from the stored bytes, so the migration is a pure backfill.
fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
         ALTER TABLE doc_changes ADD COLUMN hash TEXT;
         ALTER TABLE cook_log ADD COLUMN uid TEXT;",
    )?;
    {
        let mut stmt = conn.prepare("SELECT doc_id, seq, change FROM doc_changes")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (doc_id, seq, bytes) in rows {
            let change = Change::from_bytes(bytes)?;
            conn.execute(
                "UPDATE doc_changes SET hash = ?1 WHERE doc_id = ?2 AND seq = ?3",
                params![hex(&change.hash().0), doc_id, seq],
            )?;
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT id, date, kind, recipe, title, location, servings, verdict, tags
             FROM cook_log ORDER BY date, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, u32>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut seen: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        for (id, date, kind, recipe, title, location, servings, verdict, tags) in rows {
            let corrupt = |m: String| StoreError::Corrupt(format!("log row {id}: {m}"));
            let entry = LogEntry {
                date: date.parse().map_err(|e| corrupt(format!("bad date: {e}")))?,
                kind: kind.parse::<CookKind>().map_err(corrupt)?,
                recipe: recipe
                    .map(|s| Slug::new(s).map_err(|e| corrupt(e.to_string())))
                    .transpose()?,
                title,
                location,
                servings,
                verdict,
                tags: serde_json::from_str(&tags)?,
            };
            let prefix = log_content_hash(&entry);
            let n = seen.entry(prefix.clone()).or_insert(0);
            conn.execute(
                "UPDATE cook_log SET uid = ?1 WHERE id = ?2",
                params![format!("{prefix}-{n}"), id],
            )?;
            *n += 1;
        }
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX ux_doc_changes_hash ON doc_changes(doc_id, hash);
         CREATE UNIQUE INDEX ux_cook_log_uid ON cook_log(uid);
         PRAGMA user_version = 2;
         COMMIT;",
    )?;
    Ok(())
}

/// Relative paths of all files under `dir`, skipping `.git`.
fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .expect("walked path is under base")
                .to_string_lossy()
                .into_owned();
            out.push(rel);
        }
    }
    Ok(())
}

fn remove_empty_dirs(base: &Path, dir: &Path) -> Result<bool> {
    let mut empty = true;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            empty = false;
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if remove_empty_dirs(base, &path)? {
                std::fs::remove_dir(&path)?;
            } else {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    Ok(empty && dir != base)
}

#[cfg(test)]
mod tests {
    //! Atomicity of the store's write units, proven by denying one INSERT
    //! mid-unit through SQLite's authorizer — the same failure surface as
    //! SQLITE_BUSY, a full disk, or a kill between statements. These live
    //! inside the module because they need the raw connection to attach the
    //! hook; nothing else should.

    use super::*;
    use crate::sync::Peer;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    fn t0() -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }

    fn recipe() -> RecipeDoc {
        RecipeDoc {
            schema_version: 1,
            title: "Mapo tofu".into(),
            servings: 4,
            effort: mise_core::types::EffortClass::Weekday,
            lead: None,
            tags: Default::default(),
            equipment: vec![],
            ingredients: vec![],
            source: None,
            status: mise_core::types::RecipeStatus::Active,
            body: "".into(),
        }
    }

    /// Deny the `n`th INSERT into `table`; everything else proceeds.
    fn deny_insert(store: &Store, table: &'static str, n: u32) {
        let mut seen = 0;
        store.conn.authorizer(Some(move |ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Insert { table_name } if table_name == table => {
                seen += 1;
                if seen == n { Authorization::Deny } else { Authorization::Allow }
            }
            _ => Authorization::Allow,
        }));
    }

    fn allow_all(store: &Store) {
        store.conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    }

    /// The content hash *is* an append-only row's cross-replica identity, and
    /// `ingest_*_row` rejects any row whose recomputed hash doesn't match the
    /// uid it arrived under. So the serialization is frozen: this pins the
    /// current 16-hex prefix of a fixed row, and any change to how a
    /// `LogEntry`/`ThreadMessage` hashes — a field, a `#[serde]` attr, a type,
    /// field order — moves it and fails here, forcing the deliberate choice to
    /// version the new form into the uid rather than silently desync every
    /// existing row on every peer. Frozen values, never refreshed to match a
    /// new algorithm (that would defeat the test); a real change versions the
    /// uid and leaves these asserting the old form.
    #[test]
    fn frozen_row_identity_never_moves() {
        use mise_core::types::{CookKind, LogEntry};

        let entry = LogEntry {
            date: jiff::civil::Date::constant(2026, 7, 29),
            kind: CookKind::Meal,
            recipe: Some(slug("mapo-tofu")),
            title: "Mapo tofu".into(),
            location: "home".into(),
            servings: 4,
            verdict: "great, more numbing".into(),
            tags: BTreeMap::from([("cuisine".to_string(), "sichuan".to_string())]),
        };
        assert_eq!(log_content_hash(&entry), "5aae39fb2cac5c87");

        let message = crate::threads::ThreadMessage {
            thread: crate::threads::ThreadId::Planning,
            role: crate::threads::Role::User,
            content: "can I halve the sugar?".into(),
            created: jiff::civil::DateTime::constant(2026, 7, 29, 12, 0, 0, 0),
        };
        assert_eq!(thread_content_hash(&message), "14ea13202a68d58a");
    }

    /// Drive a full sync session; the first store initiates.
    fn pump(a: &mut Store, b: &mut Store) -> Result<()> {
        let mut pa = Peer::start(a, true)?;
        let mut pb = Peer::start(b, false)?;
        let mut msg = pa.initial_round(a)?;
        for _ in 0..64 {
            let reply = match pb.handle(b, &msg)? {
                Some(r) => r,
                None => return Ok(()),
            };
            match pa.handle(a, &reply)? {
                Some(next) => msg = next,
                None => return Ok(()),
            }
        }
        panic!("sync did not terminate");
    }

    #[test]
    fn opening_repairs_snapshots_written_from_a_stale_session_doc() {
        let dir = tempfile::tempdir().unwrap();
        let id = DocId::Pantry(slug("home"));
        let item = |name: &str| crate::pages::PantryItemDoc {
            name: name.into(),
            presence: mise_core::types::Presence::Have,
            bought: None,
            tier: None,
            note: None,
        };
        {
            let mut store = Store::create(dir.path(), &slug("home"), 2, t0()).unwrap();
            for i in 0..70 {
                store
                    .modify::<PantryDoc>(&id, "seed", t0(), |p| {
                        p.items.insert(format!("item-{i}"), item(&format!("item-{i}")));
                    })
                    .unwrap();
            }
            // Poison the cadence snapshot the way the pre-fix sync path
            // could: bytes from a doc that predates rows below the boundary.
            // An empty doc is the extreme member of that class. Then model a
            // store from before the repair: version pragma 3, no meta table.
            let mut empty = AutoCommit::new();
            store
                .conn
                .execute("UPDATE doc_snapshots SET snapshot = ?1", [empty.save()])
                .unwrap();
            store.conn.execute("DROP TABLE meta", []).unwrap();
            store.conn.pragma_update(None, "user_version", 3).unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        let pantry: PantryDoc = store.get(&id).unwrap();
        assert_eq!(pantry.items.len(), 70, "the repair rebuilt the snapshot from the intact rows");
    }

    #[test]
    fn the_connection_is_tuned_for_a_second_process() {
        let dir = tempfile::tempdir().unwrap();
        Store::create(dir.path(), &slug("home"), 2, t0()).unwrap();
        let store = Store::open(dir.path()).unwrap();
        // The design supports a CLI beside the server on one file; these
        // three are the difference between "waits briefly" and an immediate
        // "database is locked" mid-operation.
        let mode: String =
            store.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        let timeout: i64 =
            store.conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0)).unwrap();
        assert!(timeout >= 1000, "busy_timeout is {timeout}");
        let fk: i64 = store.conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn corpus_tolerates_a_location_missing_a_sibling_doc() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(dir.path(), &slug("home"), 2, t0()).unwrap();
        store.add_location(&slug("cabin"), 2, "test", t0()).unwrap();
        store
            .modify::<PantryDoc>(&DocId::Pantry(slug("cabin")), "test", t0(), |p| {
                p.items.insert(
                    "miso".into(),
                    crate::pages::PantryItemDoc {
                        name: "miso".into(),
                        presence: mise_core::types::Presence::Have,
                        bought: None,
                        tier: None,
                        note: None,
                    },
                );
            })
            .unwrap();
        store
            .modify::<EquipmentDoc>(&DocId::Equipment(slug("cabin")), "test", t0(), |e| {
                e.items.insert("wok".into(), "carbon steel".into());
            })
            .unwrap();

        // A partial sibling set — reachable from a kill between the four
        // per-doc creates, or a legacy torn state. Fabricated directly:
        // the state is defined at the row level.
        let drop_doc = |store: &Store, key: &str| {
            store.conn.execute("DELETE FROM doc_changes WHERE doc_id = ?1", [key]).unwrap();
            store.conn.execute("DELETE FROM doc_snapshots WHERE doc_id = ?1", [key]).unwrap();
            store.conn.execute("DELETE FROM docs WHERE id = ?1", [key]).unwrap();
        };

        // Missing shops: the location still reads, its pantry legible.
        drop_doc(&store, "location/cabin/shops");
        let corpus = store.corpus().expect("a missing sibling degrades, not erases");
        assert!(corpus.locations["cabin"].pantry.items.contains_key("miso"));
        assert!(corpus.locations["cabin"].shops.tiers.is_empty());

        // #11: location_view feeds readiness, /api/queue, queue_status and
        // every chat turn, and used to 500 on the same missing sibling that
        // corpus() and render_page degrade. It must degrade too.
        let view = store
            .location_view(&slug("cabin"))
            .expect("location_view degrades a missing sibling instead of erroring");
        assert_eq!(view.name, "cabin");
        assert!(view.tiers.is_empty());
        assert!(view.pantry.contains_key(&slug("miso")));

        // Missing pantry: the location is still enumerated (union of the
        // four kinds), so its equipment stays legible in the export instead
        // of being deleted as a stale file.
        drop_doc(&store, "location/cabin/pantry");
        let corpus = store.corpus().unwrap();
        assert!(corpus.locations.contains_key("cabin"), "location gone from the corpus");
        let files = crate::render::render(&corpus);
        assert!(
            files.keys().any(|k| k.starts_with("locations/cabin/")),
            "cabin no longer legible anywhere in the export: {:?}",
            files.keys()
        );
    }

    #[test]
    fn a_create_that_fails_midway_leaves_no_trace() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(dir.path(), &slug("home"), 2, t0()).unwrap();
        let id = DocId::Recipe(slug("mapo-tofu"));

        deny_insert(&store, "doc_changes", 1);
        store
            .create_doc(&id, &recipe(), "test", t0())
            .expect_err("the change insert was denied");
        allow_all(&store);

        // A doc row with no change rows is a doc no read can hydrate; if the
        // failed create left one behind, every read dies and the retry below
        // bounces off Exists — the store cannot repair itself.
        assert!(!store.exists(&id).unwrap(), "the failed create left a torn doc behind");
        store.corpus().expect("every read still works after a failed create");
        store.create_doc(&id, &recipe(), "test", t0()).expect("the same create works on retry");
        assert_eq!(store.get::<RecipeDoc>(&id).unwrap().title, "Mapo tofu");
    }

    #[test]
    fn a_sync_round_that_fails_midway_persists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Store::create(&dir.path().join("a"), &slug("home"), 2, t0()).unwrap();
        let mut b = Store::create_bare(&dir.path().join("b")).unwrap();
        pump(&mut a, &mut b).unwrap();

        a.add_location(&slug("cabin"), 2, "test", t0()).unwrap();

        // The next session carries five changes, one per doc, persisted in
        // id order: the four cabin docs, then state. Denying the fourth cuts
        // the round after pantry/cabin and before shops/cabin — the torn
        // sibling set corpus() cannot read.
        deny_insert(&b, "doc_changes", 4);
        pump(&mut a, &mut b).expect_err("the round was cut short");
        allow_all(&b);

        b.corpus().expect("reads survive an interrupted sync round");

        // The next session delivers the location whole, and both sides agree.
        pump(&mut a, &mut b).unwrap();
        assert_eq!(
            crate::render::render(&a.corpus().unwrap()),
            crate::render::render(&b.corpus().unwrap()),
        );
    }
}
