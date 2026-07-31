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
    CorpusState, EquipmentDoc, FactsDoc, FridgeDoc, LocationDocs, PantryDoc, QueueDoc,
    ShoppingDoc, ShopsDoc, StateDoc, SteeringDoc,
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
PRAGMA user_version = 3;
";

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Content-addressed identity prefix for a log row: append-only rows have no
/// CRDT, so cross-replica dedupe keys on content. The full uid is
/// `<hash16>-<n>` where `n` disambiguates genuinely repeated identical cooks.
fn log_content_hash(e: &LogEntry) -> String {
    use sha2::{Digest, Sha256};
    let canonical = serde_json::to_string(e).expect("log entries serialize");
    hex(&Sha256::digest(canonical.as_bytes()))[..16].to_string()
}

/// Same scheme for thread messages: content-hash prefix, occurrence suffix.
fn thread_content_hash(m: &ThreadMessage) -> String {
    use sha2::{Digest, Sha256};
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
        conn.execute_batch(SCHEMA)?;
        let store = Store { conn, root: root.to_path_buf() };
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
        migrate(&conn)?;
        Ok(Store { conn, root: root.to_path_buf() })
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
        let snapshot: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT upto_seq, snapshot FROM doc_snapshots
                 WHERE doc_id = ?1 ORDER BY upto_seq DESC LIMIT 1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (from_seq, mut doc) = match snapshot {
            Some((upto, bytes)) => (upto, AutoCommit::load(&bytes)?),
            None => (0, AutoCommit::new()),
        };
        let mut stmt = self.conn.prepare(
            "SELECT change FROM doc_changes WHERE doc_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let changes = stmt
            .query_map(params![&key, from_seq], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let changes = changes
            .into_iter()
            .map(Change::from_bytes)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        doc.apply_changes(changes)?;
        Ok(doc)
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
        self.append_changes(key, &[change], doc)?;
        Ok(())
    }

    /// Append changes to a doc's history, deduplicating by change hash —
    /// sync can deliver a change along more than one path. `doc` must
    /// already contain the changes (it supplies snapshot bytes on cadence).
    /// Returns how many rows were actually new.
    pub(crate) fn append_changes(
        &mut self,
        key: &str,
        changes: &[Change],
        doc: &mut AutoCommit,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM doc_changes WHERE doc_id = ?1",
            [key],
            |r| r.get::<_, i64>(0),
        )?;
        let mut inserted = 0;
        for change in changes {
            let hash = hex(&change.hash().0);
            let known: Option<i64> = tx
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
            tx.execute(
                "INSERT INTO doc_changes (doc_id, seq, hash, change) VALUES (?1, ?2, ?3, ?4)",
                params![key, seq, hash, change.raw_bytes()],
            )?;
            inserted += 1;
            if seq % SNAPSHOT_EVERY == 0 {
                tx.execute(
                    "INSERT INTO doc_snapshots (doc_id, upto_seq, snapshot) VALUES (?1, ?2, ?3)",
                    params![key, seq, doc.save()],
                )?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Make sure a doc row exists (sync may introduce docs we've never seen).
    pub(crate) fn ensure_doc_row(&self, id: &DocId) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO docs (id, kind) VALUES (?1, ?2)",
            params![id.to_string(), id.kind()],
        )?;
        Ok(())
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
        self.conn.execute(
            "INSERT INTO docs (id, kind) VALUES (?1, ?2)",
            params![key, id.kind()],
        )?;
        if committed.is_some() {
            self.persist_change(&key, &mut doc)?;
        }
        Ok(())
    }

    /// Hydrate, mutate, reconcile, persist. Returns the new value. A no-op
    /// mutation writes nothing.
    pub fn modify<T: Hydrate + Reconcile>(
        &mut self,
        id: &DocId,
        provenance: &str,
        at: Timestamp,
        f: impl FnOnce(&mut T),
    ) -> Result<T> {
        let mut doc = self.load_doc(id)?;
        let mut value: T = hydrate(&doc)?;
        f(&mut value);
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

    /// Append a cook. The row's uid is its content hash plus an occurrence
    /// index, so the same cook logged on two devices dedupes on sync while a
    /// genuinely repeated identical cook stays two rows.
    ///
    /// A first cook promotes a draft recipe to active — that rule lives here
    /// so no caller can log a cook and forget it. Promotion is a doc change
    /// (stamped with `provenance`/`at`) that syncs like any other; the log
    /// row itself is clockless. The sync insert path does not promote — the
    /// origin device already did, and its doc change is on the way.
    pub fn append_log(&mut self, e: &LogEntry, provenance: &str, at: Timestamp) -> Result<String> {
        let prefix = log_content_hash(e);
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cook_log WHERE uid LIKE ?1 || '-%'",
            [&prefix],
            |r| r.get(0),
        )?;
        let uid = format!("{prefix}-{n}");
        self.insert_log_row(&uid, e)?;
        if let Some(slug) = &e.recipe {
            let id = DocId::Recipe(slug.clone());
            if self.exists(&id)? {
                self.modify::<crate::pages::RecipeDoc>(&id, provenance, at, |r| {
                    if r.status == "draft" {
                        r.status = "active".to_string();
                    }
                })?;
            }
        }
        Ok(uid)
    }

    /// Idempotent insert of a log row with a known uid (the sync path).
    pub(crate) fn insert_log_row(&mut self, uid: &str, e: &LogEntry) -> Result<bool> {
        let inserted = self.conn.execute(
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

    pub(crate) fn log_uids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT uid FROM cook_log ORDER BY uid")?;
        let uids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(uids)
    }

    /// All log rows with their uids, in (date, uid) order.
    pub(crate) fn log_rows(&self) -> Result<Vec<(String, LogEntry)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT uid FROM cook_log ORDER BY date, uid")?;
        let uids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        Ok(uids.into_iter().zip(self.log_entries()?).collect())
    }

    /// The whole log, ordered by (date, uid) — deterministic across replicas.
    pub fn log_entries(&self) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT date, kind, recipe, title, location, servings, verdict, tags
             FROM cook_log ORDER BY date, uid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, u32>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(date, kind, recipe, title, location, servings, verdict, tags)| {
                let corrupt = |m: String| StoreError::Corrupt(format!("log row: {m}"));
                Ok(LogEntry {
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
                })
            })
            .collect()
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
            DocId::Recipe(_) => {
                let value: crate::pages::RecipeDoc = hydrate(&old)?;
                self.modify::<crate::pages::RecipeDoc>(id, provenance, at, |r| {
                    r.schema_version = value.schema_version;
                    r.title = value.title;
                    r.servings = value.servings;
                    r.effort = value.effort;
                    r.lead = value.lead;
                    r.tags = value.tags;
                    r.equipment = value.equipment;
                    r.ingredients = value.ingredients;
                    r.status = value.status;
                })?;
                let old_body = {
                    let value: crate::pages::RecipeDoc = hydrate(&old)?;
                    value.body.as_str().to_string()
                };
                self.update_body(id, &old_body, provenance, at)
            }
            DocId::Technique(_) => {
                let value: crate::pages::TechniqueDoc = hydrate(&old)?;
                let old_body = value.body.as_str().to_string();
                self.modify::<crate::pages::TechniqueDoc>(id, provenance, at, |t| {
                    t.schema_version = value.schema_version;
                    t.title = value.title;
                    t.tags = value.tags;
                })?;
                self.update_body(id, &old_body, provenance, at)
            }
        }
    }

    fn revert_plain<T: Hydrate + Reconcile>(
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
        let content = content.replace("\r\n", "\n").replace('\r', "");
        let content = content.trim();
        if content.is_empty() {
            return Err(StoreError::Invalid("empty thread message".into()));
        }
        let msg = ThreadMessage {
            thread: thread.clone(),
            role,
            content: content.to_string(),
            created,
        };
        let prefix = thread_content_hash(&msg);
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM thread_messages WHERE uid LIKE ?1 || '-%'",
            [&prefix],
            |r| r.get(0),
        )?;
        let uid = format!("{prefix}-{n}");
        self.insert_thread_row(&uid, &msg)?;
        Ok(uid)
    }

    /// Idempotent insert of a thread row with a known uid (the sync path).
    pub(crate) fn insert_thread_row(&mut self, uid: &str, m: &ThreadMessage) -> Result<bool> {
        let inserted = self.conn.execute(
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

    /// Hydrate everything: the render layer's input.
    pub fn corpus(&self) -> Result<CorpusState> {
        let mut locations = BTreeMap::new();
        for id in self.list("pantry")? {
            let DocId::Pantry(loc) = id else { unreachable!() };
            let docs = LocationDocs {
                pantry: self.get::<PantryDoc>(&DocId::Pantry(loc.clone()))?,
                equipment: self.get::<EquipmentDoc>(&DocId::Equipment(loc.clone()))?,
                shops: self.get::<ShopsDoc>(&DocId::Shops(loc.clone()))?,
                fridge: self.get::<FridgeDoc>(&DocId::Fridge(loc.clone()))?,
            };
            locations.insert(loc.as_str().to_string(), docs);
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

    /// The plain view of one location, for readiness and coverage.
    pub fn location_view(&self, location: &Slug) -> Result<LocationView> {
        let state: StateDoc = self.get(&DocId::State)?;
        let meta = state
            .locations
            .get(location.as_str())
            .ok_or_else(|| StoreError::NotFound(format!("location {location}")))?;
        let docs = LocationDocs {
            pantry: self.get(&DocId::Pantry(location.clone()))?,
            equipment: self.get(&DocId::Equipment(location.clone()))?,
            shops: self.get(&DocId::Shops(location.clone()))?,
            fridge: self.get(&DocId::Fridge(location.clone()))?,
        };
        docs.to_view(location.as_str(), meta)
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
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    match version {
        3 => Ok(()),
        2 => migrate_v2_to_v3(conn),
        1 => {
            migrate_v1_to_v2(conn)?;
            migrate_v2_to_v3(conn)
        }
        other => Err(StoreError::Corrupt(format!(
            "unsupported schema version {other} (this build understands 1..=3)"
        ))),
    }
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
