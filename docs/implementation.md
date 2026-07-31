# Implementation plan

*Last updated: 2026-07-31 (M6 build). Companion to [design.md](design.md); this document
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

Settled 2026-07-29, at M3 build start:

- **Threads are log-shaped, and they sync.** One thread per page plus the
  global planning thread, stored as append-only rows with the log's
  content-hash uid identity (`sha256(msg)[..16]-<n>`), merged by the same
  one-time uid exchange in the sync protocol. Ordering is (created, uid);
  the reply to a message is stamped by a second clock reading (clamped
  monotone) so transcripts sort in conversation order. Threads hold **text
  turns only** — tool activity is not transcribed; edits carry provenance
  in page history, and a resumed conversation re-reads pages through tools
  rather than trusting stale tool results. Transcripts export to
  `threads/<thread-id>.md` (`threads/planning.md`,
  `threads/recipe/mapo-tofu.md`) with blockquoted content, covered by the
  export determinism/completeness properties. Schema v3 (v2's unreachable
  thread tables drop and recreate).
- **The seam, concretely.** `mise-assistant` is its own crate. The `Model`
  trait is the seam: `next_turn(request, on_delta) → ModelTurn` over our
  own content-block types. The tool loop is a sans-IO `Turn` state machine
  (like sync's `Peer`): callers shuttle model turns and tool outcomes, so
  lock discipline is theirs — the server holds its store mutex only around
  store work, never across model calls. `run_exchange` packages the flow
  for exclusive-store callers (CLI, tests).
- **Tools.** Nineteen deterministic operations mirroring the CLI surface —
  reads (queue_status with readiness/coverage, list_pages, read_page by
  export path, search), queue/recipe/pantry/equipment/fridge/log/shopping
  edits, steering_set/facts_set. No export inside tools; one export per
  exchange, provenance `planning thread: …` / `thread recipe/x: …`.
  Model-recoverable problems (bad input, unknown slug, duplicate) return
  as `is_error` tool results; real store failures abort the exchange.
- **Context assembly.** System prompt layered for prompt caching:
  instructions, then slow-moving corpus context (state/steering/facts,
  plus the page for page threads), the clock dead last — a test pins that
  changing the clock only changes the final line. History is the thread's
  text turns. `DocId::export_path()` is the single authority mapping docs
  to export paths.
- **Anthropic client.** Hand-rolled over `reqwest` (rustls); pinned
  `anthropic-version: 2023-06-01`; streaming SSE with an incremental
  framer and turn assembler (both pure, unit-tested); `cache_control`
  markers on the system block and last tool; image content blocks mapped
  and ready for M5. Default model `claude-opus-5`, overridable
  (`mise chat --model`, `services.mise.model`).
- **Surfaces.** CLI: `mise chat "…" [--page recipe/x]` streams text to
  stdout and tool activity to stderr. Server: `POST /chat` (bearer-authed)
  streams SSE `delta`/`tool`/`done`/`error` events; without an Anthropic
  key the server runs sync-only and /chat answers 503. The key arrives
  like the bearer token: `--anthropic-key-file`, then
  `$CREDENTIALS_DIRECTORY/anthropic` (module option `anthropicKeyFile`,
  agenix-friendly), then `$ANTHROPIC_API_KEY` (dev `.env`).
- **Evals.** `evals/` is a workspace binary (`cargo run -p mise-evals`) so
  it compiles in CI but only runs by hand against the real API. Scenarios
  seed a lived-in corpus and score mechanical checks (explored before
  proposing, reasons on the queue, pantry/log/fridge actually changed);
  transcripts print for human judgment.

Settled 2026-07-30, at M4 build start:

- **The clock threads into mutations.** Every mutating store API takes a
  `jiff::Timestamp`, stamped into the Automerge change beside the
  provenance message — the store still never reads a clock; edges pass
  `Zoned::now()`, tests script it. Sync carries original times for free
  (they live in the change bytes). Changes from pre-clock builds show no
  time, forever — they're immutable.
- **Revert semantics.** `Store::history(doc)` lists (hash, message, time)
  per change; `Store::revert(doc, hash)` restores the page to its state
  as of that change, recorded as a *new forward change* — history only
  grows, and a revert is itself visible and revertible. Prose bodies
  restore through the char-safe splice path. Property: revert reaches
  every point in history exactly.
- **The JSON API.** Bearer-authed under `/api`: `queue` (the readiness/
  coverage/someday view — one structured type in `mise-assistant::views`
  that both the tool string and the JSON render from), `pages` (browse
  metadata + doc handles), `page/{path}`, `history/{doc}`, `revert`
  (POST), `thread/{id}`. Mutations beyond revert stay conversational
  (/chat) or CLI — the API is not a second editing surface.
- **The web app.** SvelteKit static-adapter SPA (Svelte 5 + TS +
  Pico.css, npm with committed lockfile), served whole by `mise-server
  --static-dir` with index.html fallback. Queue home + planning thread;
  generic page view (frontmatter as metadata chips, markdown via marked)
  with recent-changes/revert and the page's thread; browse by tag chips.
  One token prompt, localStorage, 401 loops back to it. Chat streams over
  fetch with a TS SSE framer mirroring the Rust one, vitest-covered.
- **E2E.** Playwright (`npm run e2e` in `web/`) drives the planning-
  session flow against the real server binary and a scripted fake
  Anthropic endpoint (`mise-server --anthropic-base-url`); deterministic,
  no model, run manually like the browsers it needs.
- **Packaging.** `packages.web` via `buildNpmPackage` (offline, locked);
  the NixOS module serves it by default (`services.mise.webApp`, null for
  sync/API-only); `nodejs_24` joins the devshell.

Settled 2026-07-30, after M4 (planning M5 and beyond):

- **Typed mutations narrow — not reverse — "not a second editing
  surface."** Direct edits in the UI are a design promise (update path
  #4); they arrive as a small set of typed endpoints calling exactly the
  store operations the assistant's tools and CLI already use — add/remove
  an equipment item, set a pantry entry, check off a list item. No
  free-text document PUT; prose bodies stay conversational. Endpoints
  are *tap-shaped* — small, idempotent, timestamped ops — so the M9
  offline queue is a replay buffer, not a rewrite.
- **Recipe status is an enum.** `draft` / `active` / `retired` replaces
  the retired flag. Drafts (curiosity, a URL worth keeping) show on the
  cookbook's drafts shelf but stay out of steering rotation until a
  first cook promotes them.
- **`fetch_url` tool.** One deliberate URL at a time — the bulk-import
  non-goal stands. Server-side pipeline: schema.org `Recipe` JSON-LD
  when present (most recipe sites; exact ingredients/steps, zero life
  story), else Readability extraction (`dom_smoothie`) rendered to
  Markdown (`htmd`). Size cap, timeout, http(s) only, private ranges
  blocked. The pipeline is deterministic — fixture-HTML tests in the
  suite; draft quality from messy pages is an eval.
- **Header nav and a real cookbook page.** Persistent nav: Queue,
  Cookbook, the active location's standing pages (equipment, pantry).
  The cookbook page is the app's face — recipes by the browse axes, a
  drafts shelf, and a new-recipe box on its own **drafting thread**
  (`ThreadId::Drafting`): a page thread can't exist before its page
  does, and planning shouldn't collect drafting chatter. The box shows
  the current session (a reply that ends in a question stays on
  screen), links fresh drafts, and the full transcript exports to
  threads/drafting.md like any thread. The current everything-list
  survives as a debug corner.
- **Friends fork, they don't share.** Tenancy is one corpus per person:
  the NixOS module grows an `instances` attrset (own root, token, port,
  per-instance Anthropic key), Caddy routes per name, and "forking" is
  copying a corpus or `--init`. No app code — every corpus stays
  single-user, so the multi-user non-goal survives untouched. Built when
  the first friend asks, not before.
- **PWA lands last.** The install shell is trivial; the pride-worthy
  part is offline data, and that work is cheapest once the client has
  stopped moving. Store mode ships online-first; offline + install
  becomes the closing milestone.

Settled 2026-07-30, at M5 build:

- **No status migration, no compat shim.** The status enum replaced the
  `retired` bool before any corpus existed outside development, so the
  pre-enum doc shape simply never shipped: no migrator, no tolerant
  hydrator, nothing to go wrong. (Post-deployment schema changes will
  need the tolerant-hydrate treatment — revert hydrates *historical*
  doc states, which a one-shot migration can never fix.)
- **First-cook promotion lives in `Store::append_log`.** The signature
  grew provenance + timestamp; no caller can log a cook and forget the
  rule. The sync insert path doesn't promote — the origin device did,
  and its doc change is in flight.
- **`/api/edit/{action}` executes the assistant's own tools.** An
  allowlist (pantry, equipment, fridge, shopping, recipe-status) maps
  each action to the matching tool under `ui:` provenance — same
  validation, same normalization, then an export. `recipe-status`
  forwards only `slug` + `status`, so no payload smuggles free text.
- **`/api/location` is the editors' read path.** Item editors consume
  the structured active-location view; the markdown export is rendered,
  linked, and never parsed by the app. Editors on a non-active
  location's page stay hidden until the M8 location selector.
- **`Fetch` is a seam like `Model`.** Drivers intercept `fetch_url` and
  run it outside the store lock; tests and evals script the network
  (the eval fixture is a life-story page with no JSON-LD). `HttpFetch`
  re-validates every redirect hop: http(s) only, private addresses and
  local hostnames refused, 20 s budget, 2 MB cap.
- **Drafted-from-somewhere is structural.** `RecipeDoc.source` holds
  the URL a page was drafted from; the export renders it in
  frontmatter, the web app links it. And when a fetch returns a
  recipe's shape without its substance (a client-side calculator, a
  paywall), the prompt says ask, don't invent — the gap goes in the
  reply as a question, never in the page as a caveat. Both are evals
  (`draft-from-url`, `calculator-page`).

Settled 2026-07-30, at M6 build:

- **Pantry recon before store mode.** The nearest store is a 40-minute
  drive; a milestone that can't be exercised can't be trusted. Photo recon
  is the shared machinery behind the store-shelf snap, the arrival ritual,
  and first-run bootstrapping — built first against the home pantry, where
  it gets tested daily, it leaves store mode (M7) as a thin surface over
  proven parts.
- **Recon proposes; the user applies.** Misreads are the expected case —
  invented jars, missed bags — so a photo never edits the pantry.
  `propose_pantry_diff` is intercepted by the drivers like `fetch_url`,
  validated, forwarded to the UI, and *never touches the store*. Each
  proposal line is exactly one `pantry-set` tap on the existing edit
  endpoints; the whole-proposal correction path ("you missed the rice;
  that's gochujang, not miso") is free text on the same thread, and the
  prompt ranks it above the photo, which ranks above the page.
- **Photos are conversation input, not corpus state.** The image block
  rides only the live exchange; the stored thread turn carries a
  `[photo attached]` placeholder and the assistant's reply summarizes what
  it proposed, so the transcript stands alone. Nothing binary enters the
  store, sync, or the export — the applied taps are what endure. (Debrief
  photos on the log are a separate, unbuilt question.)
- **Recon quality is an eval; recon photos are private by default.** The
  scripted suite covers everything below the seam (validation, events,
  taps). The judgment call — what the model sees on a real shelf — runs
  as an eval over `evals/fixtures/shelves/` (a checked-in set the user
  explicitly cleared for the repo, EXIF stripped, including a
  `not-a-shelf-*` robustness case the model must decline) plus anything
  in the gitignored `evals/fixtures/private/`, the default drop zone for
  photos nobody has cleared.

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
    threads/<thread-id>.md      # transcripts: planning.md, recipe/<slug>.md, …
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

Per the testing charter, all model interaction sits behind one trait
(built in M3 as `mise_assistant::seam::Model`):

```rust
trait Model {
    // given the system prompt, conversation, and tool definitions,
    // produce the next turn: text and/or tool calls, streamed
    async fn next_turn(&mut self, req: &TurnRequest, on_delta: …) -> ModelTurn;
}
```

Below the seam: tool implementations (search corpus, read page, edit page,
append log, update pantry, ...) are ordinary deterministic functions over the
store, and the tool loop itself is a sans-IO `Turn` state machine — both
tested with scripted fakes. Above the seam: the real client calls the
Messages API and streams back. Prompt/judgment quality lives in a
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
  assistant/   # LLM seam trait + tools + context assembly + turn driver
               # + the hand-rolled Anthropic client
  server/      # axum: WS sync + SSE chat streaming + (M4) static files
  cli/         # the `mise` binary: local corpus operations + `mise chat`

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

**M5 — Cookbook & direct hands.** Header nav (Queue, Cookbook, standing
pages); the cookbook page (recipes by metadata axes, drafts shelf,
new-recipe box on the drafting thread); recipe status enum; typed
tap-shaped mutation endpoints with in-place item editors on
equipment/pantry; `fetch_url` tool (JSON-LD → Readability → Markdown
pipeline, fixture tests, messy-page drafting evals). Deliverable: draft a
recipe from a URL, and fix the equipment list without opening a
conversation.

**M6 — Pantry recon.** Photo recon pointed at the home pantry — the shared
machinery for the store-shelf snap (M7) and the cottage arrival ritual (M8),
built first against the shelves that get looked at daily. Photo capture on
pantry-page threads (client-side downscale), image blocks through the Model
seam, and the `propose_pantry_diff` tool: the assistant *proposes* a diff,
never applies it — misreads are the expected case, so accepted lines are
per-line taps onto the existing edit endpoints, and the correction path is
free text on the same thread ("you missed the rice; that's gochujang, not
miso"), which outranks the photo. Photos are conversation input, not corpus
state: the thread stores a placeholder, the applied taps are what endure.
Deliverable: snap the pantry shelf, tap the diff into truth.

**M7 — Store mode, online-first.** Tiered shopping list with
bought/unfindable taps, M6 recon pointed at store shelves, terse store-mode
assistant surface. Deliverable: the store flow, with connectivity.

**M8 — Locations & trip prep.** Location selector (sticky, confirmed),
per-location readiness/coverage, bring-from-home packing list, arrival
recon ritual (M6 machinery), confidence decay for secondary locations.
Deliverable: the cottage flow.

**M9 — Offline & install.** PWA manifest + service worker + IndexedDB,
offline tap queue replaying through sync, install polish. Deliverable: the
store flow again, airplane mode included.

First-run bootstrapping (seeding from project.txt, photo recon of shelves)
is M3+M6 machinery pointed at an empty corpus — it needs no dedicated
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
