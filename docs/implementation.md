# Implementation plan

*Last updated: 2026-07-29. Companion to [design.md](design.md); this document
covers the technical shape and build order. Decisions here resolve the "Open
questions" section of the design doc.*

## Decisions

Settled in discussion 2026-07-27:

- **Backend: Rust.** Single binary, systemd service on the existing
  NixOS/Caddy host. Caddy terminates TLS (HTTP/3 included — no separate QUIC
  path; the clients are browsers). Cloudflare optional, orthogonal.
- **LLM access: hand-rolled client** over the Anthropic Messages API. No
  community crate. Scope: messages, SSE streaming, the tool-use loop, image
  content blocks, prompt caching headers. Default model `claude-opus-5`
  everywhere to start; per-surface tiering (cheaper/faster model for
  store-mode taps) is a config knob to tune later, not an architecture
  decision.
- **CRDT: Automerge** (`automerge` crate, with `autosurgeon` for typed
  struct mapping). Same-org, maintained, Rust-native, and its change history
  maps directly onto the design's "recent changes / revertible" requirement.
- **Frontend: SvelteKit** (static adapter) + TypeScript, as a PWA. Chosen for
  testability (Vitest for logic, Playwright for flows) and a mature service-
  worker story for offline store mode. Swappable if it disappoints — the
  server exposes a plain API and knows nothing about the frontend.
- **This repo is the MIT-licensed application.** The corpus is private data
  living at `$HOME/cookbook` on the server, never in this repo.
- **Build order: inside-out.** Deterministic core first, fully tested with no
  UI and no model; surfaces layer on top.

Settled 2026-07-29:

- **The store is the truth; markdown is an export.** All corpus state lives
  in a single SQLite file; the markdown tree is a deterministic, read-only
  export, regenerated on change and committed to a local git repo. It is
  never read back — there is no hand-edit import path; the app and the
  assistant are the only writers. This deletes the file watcher, hand-edit
  reconciliation, and the malformed-edit policy from the plan, and makes the
  merge machinery an internal detail of `store/`, swappable without breaking
  any promised format.

Settled 2026-07-29, ahead of M1:

- **Ingredient ↔ pantry identity: explicit slug links.** An ingredient line
  optionally names a pantry-item slug; the assistant maintains links from M3
  on. Unlinked ingredients surface honestly in readiness as "unknown — needs
  linking" — never guessed by name matching.
- **Coverage counts dinners.** Coverage = cooked servings ÷ the location's
  headcount, one dinner per day. Lunches depleting servings early is slop
  the presence-and-rough-date model already tolerates.
- **M1 schema details.** Lead time is explicit recipe metadata — a duration
  plus a named act-now step; no step-graph inference. Effort class is a
  two-value enum (weekday / project). Rotation axes are free-form
  frontmatter tags (cuisine / protein / format by convention); rotation math
  runs over whatever tags exist. Automerge docs persist as append-only
  change rows plus periodic snapshots in SQLite. Every doc carries a
  schema-version field from day one. The location registry — names plus
  per-location headcount — lives on the state page; other per-place standing
  facts join it when they show up. The queue renders in (added, id) order
  for now; a real user-ordering affordance (fractional position) waits for
  the M4 UI that can express it.

Settled 2026-07-29, at M1 build start:

- **Library choices.** `jiff` for dates and durations (civil dates fit the
  presence-and-rough-date model; the clock-as-parameter design matches the
  testing charter), `proptest` for property tests, `rusqlite` (bundled) for
  SQLite, `clap` for the CLI. Confirmed with the user.
- **The CLI is its own crate**, `crates/cli`, building the `mise` binary. It
  is an edge: it reads the wall clock and passes it into `core` as data.

Settled 2026-07-29, at M2 build start:

- **Sync protocol: JSON rounds over one WebSocket, strict alternation.**
  Each round carries base64 Automerge sync messages tagged by doc id (docs
  new to one side are created by sync), plus a one-time exchange of log-row
  uids followed by whichever entries the other side lacks. The initiator
  says `done` after an empty round in both directions; the responder echoes
  it. Every round is persisted before replying, so an interrupted sync
  loses nothing. The peer machinery is sans-IO in `store/` — server, CLI,
  and tests drive the identical code; the transport is dumb pipe.
- **Log-row identity: content hash + occurrence index.** Append-only rows
  have no CRDT, so cross-replica dedupe keys on content: uid =
  `sha256(entry)[..16]-<n>`. The same cook logged on two devices merges to
  one row; a genuinely repeated identical cook is `-0`, `-1`. Log ordering
  is (date, uid) — deterministic across replicas, so converged replicas
  still export byte-identically. Change rows likewise carry their Automerge
  change hash, deduping changes that arrive via two paths. Schema v2; v1
  databases migrate on open by pure backfill.
- **Auth.** One static bearer token (≥16 chars), `Authorization: Bearer` or
  `?token=` for browser clients that can't set WS headers; constant-time
  compare. Server reads it from `--token-file`, systemd
  `$CREDENTIALS_DIRECTORY/token`, or `$MISE_TOKEN`. In production the
  token file is an agenix secret fed through `LoadCredential`; in dev,
  both binaries load a git-ignored `.env` via dotenvy, so `MISE_TOKEN`
  and `MISE_ROOT` come from there. Clients store the token in
  `remote.json` (0600) beside the corpus — never in the export.
- **Server defaults.** `mise-server` binds 127.0.0.1:7920; Caddy proxies
  and terminates TLS. `--init` creates the corpus on first start. Client
  join flow: `mise init --from <url> --token …` = bare corpus + saved
  remote + first sync. Packaging: `flake.nix` (package + devshell) and
  `nix/module.nix` (`services.mise`, hardened systemd unit, git on the
  service path, token via LoadCredential).

## Architecture

```
┌────────────────────────── server (Rust, systemd) ──────────────────────────┐
│                                                                            │
│  corpus store ── Automerge docs in SQLite (truth) → markdown export       │
│       │                                                                    │
│  domain core ── readiness, coverage, lead time, rotation (pure, clocked)   │
│       │                                                                    │
│  assistant ──── tool loop over the corpus (behind the LLM seam)            │
│       │                                                                    │
│  HTTP API ───── REST + WebSocket (automerge sync, chat streaming)          │
│                 + serves the built PWA                                     │
└────────────────────────────────────────────────────────────────────────────┘
          ▲ HTTPS via Caddy (h2/h3)
   desktop browser · phone PWA (IndexedDB + service worker, offline-capable)
```

### The page model: the store is truth, markdown is an export

Every page is an Automerge document, persisted in a single SQLite file
(`mise.db`) alongside threads, the log, and blob metadata. The store is the
truth. The markdown tree is a deterministic, **read-only export**:
regenerated from the docs after every change, committed to a local git repo
(browsable history for free), and never read back. All edits — assistant
tool calls and the UI's raw-edit affordance alike — go through the store, so
every change carries provenance and is revertible (Automerge changes *are*
the history).

Two page classes, different merge granularity:

| Class | Pages | Representation | Merge behavior |
|---|---|---|---|
| **Structured** | pantry, shopping list, queue, fridge/freezer state, steering, standing facts, equipment | Automerge **map** keyed by item/entry, typed via autosurgeon | Item-level merges: "checked off eggs" ⊕ "miso is out" compose trivially and can never garble the file |
| **Prose** | recipes, techniques | Automerge **text** + structured frontmatter/metadata map | Character-level text merge; concurrent prose edits are rare and mostly assistant-mediated |

Serialization stays deterministic — same doc state → byte-identical markdown
— but its job has shrunk: it keeps export diffs clean and git history
readable rather than underpinning a reimport path. Property-tested per the
testing charter, alongside export completeness: everything in the store is
legible somewhere in the export, verified by a test-only parser (see the
testing charter in CLAUDE.md).

**Not CRDTs:** threads and the log are append-only rows in SQLite —
append-merge on sync is sufficient, and the log's unbounded growth stays out
of any text CRDT's history. Photos are a content-addressed blob directory on
disk, referenced by hash from pages and threads.

### On-disk layout (initial; expect drift)

```
~/cookbook/
  mise.db                       # the truth: pages, threads, log, history
  remote.json                   # client devices: saved sync server + token (0600)
  photos/<hash>.<ext>           # content-addressed blobs
  export/                       # read-only markdown mirror, a git repo
    queue.md                    # global — desires travel with you
    someday.md
    steering.md
    facts.md                    # standing facts / memory
    state.md                    # active location + misc global state
    log/<yyyy-mm>.md            # cook log, sharded by month
    recipes/<slug>.md
    techniques/<slug>.md
    locations/<name>/           # home, cottage, ...
      pantry.md  equipment.md  shops.md  fridge.md
    threads/<page-path>.md      # rendered transcripts + planning.md
```

`mise.db` is what gets backed up. The export is derived — deletable and
regenerable at any time — and kept as a git repo so "what did this recipe
say before March?" is answerable with `git log` as well as through the app.

The export is literally a git repository, and the app drives it by shelling
out to system git (guaranteed present via the NixOS module) — no git
library, no hand-rolled object format; the export writer's job is "write
files, `git add -A`, `git commit`". One commit per change batch, provenance
in the message ("planning thread: checked off eggs, miso out"), so
`git log -p` on any file reads as a change journal. Pushing to a private
remote is optional free offsite backup of the readable form. The git
history is a courtesy view: Automerge history in `mise.db` is authoritative,
and revert goes through the store — never `git revert`, because nothing
reads the export back.

### Sync

Clients hold Automerge docs locally (IndexedDB) and run the Automerge sync
protocol over WebSocket. Offline is not a special mode: local changes
accumulate and sync when connectivity returns. The motivating scenario —
checking off shopping items in a signal-dead basement while a desktop thread
edits the pantry — is exactly what item-level CRDT merges make convergent,
and it gets the named test the charter demands.

Auth: single user. Static bearer token issued by the server, stored by the
client after a one-time login; Caddy handles TLS. No accounts, no sessions to
expire.

### The LLM seam

Per the testing charter, all model interaction sits behind one trait:

```rust
trait Assistant {           // sketch, name TBD
    // given a thread + tool definitions, produce the next turn:
    // tool calls to execute, text to say, page edits to apply
}
```

Below the seam: tool implementations (search corpus, read page, edit page,
append log, update pantry, ...) are ordinary deterministic functions over the
store, tested with scripted `Assistant` fakes. Above the seam: the real
implementation assembles context (page + thread + relevant corpus), calls the
API with the tool loop, streams back. Prompt/judgment quality lives in a
separate `evals/` directory, run manually against the real API, never in CI.

Tool-loop details worth writing down now:

- Tools are the *same* operations the HTTP API exposes — the assistant is
  just another client of the store. No privileged side door.
- Every assistant edit records provenance (which conversation, when) into the
  page history — feeds the "recent changes" UI.
- Prompt caching: stable system prompt + corpus context first, volatile
  thread tail last. Thread summarization (design doc's "thread hygiene")
  deferred until threads actually get long; the seam makes it invisible when
  it lands.

## Workspace layout

```
crates/
  core/        # domain: types, readiness, coverage, lead time, rotation.
               # Pure. Clock is a parameter. No IO, no automerge.
  store/       # corpus: SQLite persistence, automerge docs, markdown
               # export (git-committed), history, threads, log, photos
  assistant/   # LLM seam trait + tools + context assembly + the real
               # Anthropic client (small; module, not a separate crate)
  server/      # axum: REST + WS sync + chat streaming + static files
  cli/         # the `mise` binary: local corpus operations, M1's surface

web/           # SvelteKit PWA
evals/         # prompt-quality checks, separate from the test suite
docs/
```

`core` depending on nothing and `store` depending only on `core` is the
enforcement mechanism for "time is an input" and "no model in the test
suite" — the dependency graph makes violations awkward by construction.

## Milestones

Each milestone ends green: tested, and demoable by CLI or browser.

**M1 — Corpus & core.** Page schemas, SQLite store, Automerge integration,
deterministic markdown export. Readiness (incl. equipment
and lead time), coverage math, rotation recency, effort classes. Property
tests: export determinism and completeness, CRDT convergence under
seeded interleavings, the basement-checkoff named test. Deliverable: a `mise`
CLI that can init a corpus from scratch, show the queue with readiness
annotations, and export the corpus to a browsable git repo.

**M2 — Server & sync.** Axum service, automerge sync over WebSocket, bearer
auth, NixOS module/systemd packaging, CLI grows a remote mode. Deliverable:
two clients converging through the server, offline edits included.

**M3 — Assistant.** The seam trait, tool set, context assembly, hand-rolled
Anthropic client, planning-assistant thread + per-page threads, debrief flow
(fold lessons into recipe, append log, touch pantry/fridge), pantry updates
in passing. Scripted-fake tests for all tool logic; first evals. Deliverable:
plan a week from the CLI/API against a real corpus.

**M4 — Desktop web.** Queue home (readiness, fridge state, coverage
warning), recipe/technique/pantry pages with recent-changes + revert, thread
UI with streaming, browse by metadata. Deliverable: the planning-session flow
from the design doc, end to end in a browser.

**M5 — Store mode.** PWA install + service worker + IndexedDB, tiered
shopping list with bought/unfindable taps, offline queue-and-sync, photo
capture → recon diff, terse store-mode assistant surface. Deliverable: the
store flow, airplane-mode test included.

**M6 — Locations & trip prep.** Location selector (sticky, confirmed),
per-location readiness/coverage, bring-from-home packing list, arrival
photo-recon ritual, confidence decay for secondary locations. Deliverable:
the cottage flow.

First-run bootstrapping (seeding from project.txt, photo recon of shelves)
is M3+M5 machinery pointed at an empty corpus — it needs no dedicated
milestone, just a first-run path in the assistant prompt.

## Risks / watch items

- **Export drift.** The export is only a trustworthy backup if it is
  complete and deterministic: nondeterminism (map ordering, float
  formatting) makes its git history unreadable, and anything the export
  omits silently breaks the exit-strategy promise. Mitigation: property
  tests for determinism and completeness from day one; BTreeMaps everywhere.
- **Automerge doc growth.** Fine-grained history grows without bound;
  Automerge compaction exists but interacts with history UI. Revisit when a
  pantry doc gets slow, not before — and with no on-disk format promised to
  anyone, swapping the merge machinery later is a data migration, not a
  breaking change.
- **API drift.** The Anthropic client is hand-rolled; pin the API version
  header, keep the surface minimal, and keep evals runnable so drift is
  noticed.
