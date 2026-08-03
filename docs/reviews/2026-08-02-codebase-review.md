# Mise Codebase Review — Remediation Report

_Generated 2026-08-02 by a multi-agent audit (18 Opus finder agents, one per
area; every bug/security finding then independently adversarially verified).
101 raw findings → 85 kept (45 confirmed, 1 uncertain, 39 non-bug findings that
skip the disprove pass), 1 refuted._

_Revision: jj change `zvsyutsksxvv` · commit `b7fc8a86017e`. Scope: whole codebase._

<!-- audit-revision
mode: whole
commit: b7fc8a86017e
jj-change: zvsyutsksxvv
generated: 2026-08-02
-->

Per `CLAUDE.md`, fixes want a failing regression test **first** — except where
the fix makes the bug class unrepresentable in the types, in which case say so
and skip the test rather than bending the design to keep the bug reachable.
Several findings below are exactly that second kind and are marked.

## Executive summary

This audit was run to answer one question: did the remediation campaign that
just closed (94 findings from the [2026-07-31 review](2026-07-31-codebase-review.md),
89 fixed) introduce new bugs, or leave old ones half-fixed? The answer is mostly
reassuring and specifically not. The campaign's headline fixes hold where they
landed: the markdown raw-HTML sink is escaped, the v4-mapped SSRF bypass is
closed, sync no longer writes stale snapshots, the auth tower layer is real, the
export self-heals, and `Store::revert` destructures its docs. No finding here
argues any of those was wrong. What the audit found instead is a recurring
*shape*: **a fix that landed on one of two parallel code paths and left its twin
on the old behaviour.** That pattern accounts for both HIGH findings and a large
share of the mediums.

The two HIGH findings both sit in the CLI, and both are confirmed against the
built binary:

- **`wss://` sync cannot connect at all** (#52). `tokio-tungstenite` is declared
  with default features — no TLS — while `normalize_url` deliberately maps
  `https://` → `wss://` and the documented production topology is "Caddy
  terminates TLS". Every `mise init --from https://…` fails immediately with
  "TLS support not compiled in"; the test suite never sees it because
  `tests/remote.rs` only ever talks `ws://127.0.0.1`. The workaround a user
  reaches for — a `ws://` URL for a public host — puts the bearer token on the
  wire in cleartext.
- **`mise fridge add` still allocates positional ids** (#31). The prior HIGH fix
  replaced lowest-free `p<n>` scanning with `Store::mint_id` on the *assistant
  tool* path and pinned it with a tools-level convergence test — but the CLI's
  `FridgeCmd::Add` kept the old scan. On any corpus whose portions came from the
  assistant or web UI, every CLI add on every device deterministically picks
  `p1`, and two offline adds destroy each other on merge. `remediation.md:63`
  records this class as fixed; for the CLI it is not.

Those two are the sharp edge of the audit's dominant theme:

**The CLI is a forked, unvalidated copy of the tool layer, and the campaign's
fixes reached only the copy.** `tools.rs`'s own module doc promises "the same
operations the CLI and HTTP surface expose … no privileged side door." The HTTP
surface honours this (`/api/edit` → `tools::execute`); the CLI does not. Its
`run_pantry`/`run_fridge`/`run_equipment`/`run_queue` open `store.modify` with
private validation and private copies of `slugify`/`resolve_location`/etc., and
four separate campaign fixes landed on only the tool copy: replica-safe ids
(#31), tier existence-checking (#32), servings bounds (#35), equipment-note
patching (#50), and queue upsert semantics (#51) are all still wrong on the CLI
while correct through the assistant and the web UI — against the *same synced
corpus*. This is finding #33, and #31/#32/#35/#50/#51 are its instances. The
single highest-leverage repair in this report is to route the CLI mutation
subcommands through `tools::execute`, which deletes the whole class.

Three other clusters matter:

- **Error paths that erase their own signal.** On the web client, an SSE `error`
  frame is written to `error` and wiped by the `reload()` on the very next line,
  so no server-side exchange failure ever shows a banner (#60); the recipe-status
  side fetch routes its failure into the page-level `error`, replacing an
  already-rendered recipe and its live thread with a bare ⚠ (#62); and the token
  gate stores a candidate token on *any* non-401 response, so a 500 or an
  SPA-fallback 200 locks in a bad token (#66). These are new, and they cluster in
  the same files the composer campaign touched.

- **The schema-migration machinery isn't load-bearing yet.** `schema_version` is
  never re-stamped on write, and `revert` actively rewrites it *downward* over
  new-shape bytes (#9); the compat harness is hard-wired to one version directory,
  so the "every past version stays covered forever" promise has no mechanism
  (#12); and append-only row identity hashes the *live serde shape* of
  `LogEntry`/`ThreadMessage`, so adding one `#[serde(default)]` field permanently
  kills sync between builds, with none of the frozen-fixture discipline the doc
  side has (#19). None of these bites at `SCHEMA_VERSION == 1`, but they are the
  exact machinery the schema policy was written to guarantee, and they are cheap
  now and expensive after v2.

- **Two security textual-checks and a missing egress fence.** The XSS *URL* half
  survives: `safeUrl` tests raw markdown text while the browser decodes HTML
  character references in the emitted href, so `&#106;avascript:` and `java&Tab;script:`
  reach the DOM (#57) — held off today only by the CSP, which itself has zero test
  coverage (#71). A trailing-dot hostname (`http://localhost./`) walks past the
  local-host refusal (#38). And the systemd unit the SSRF deferral explicitly
  designates as its "second line of defence" applies no `IPAddressDeny` at all
  (#76), so a hostname resolving into the LAN or to the metadata address is
  reachable exactly as with no sandbox.

The four deliberately-deferred items from last time remain correctly open and
documented: the rotation tool (#2), `slugify` non-ASCII, and the three
extraction gaps — ISO-8601 durations losing days (#42), the empty JSON-LD husk
beating Readability (#43), the ignored charset (#44), plus the SSRF
resolve-and-pin (#45) and the prose-only user-turn rule (#84). They recur here
only so the chain stays honest; do not re-litigate them.

### Fix-first order

1. 🔴 **Compile TLS into the CLI WebSocket stack** — `crates/cli/src/remote.rs:113`. Enable `tokio-tungstenite`'s `rustls-tls-webpki-roots` feature to match reqwest; add a `wss://` test that fails on handshake, never on `TlsFeatureNotEnabled`. Production sync is impossible without it (#52).
2. 🔴 **Replica-safe fridge ids on the CLI** — `crates/cli/src/main.rs:790`. Mint through `Store::mint_id("p")` as `tools::fridge_add` does, or route the command through `tools::execute`; add a CLI-level partition test asserting both portions survive a merge (#31).
3. 🟠 **Route CLI mutations through `tools::execute`** — `crates/cli/src/main.rs:778`. The structural fix that subsumes #31, #32, #35, #50, #51 and closes the "no privileged side door" gap for good (#33). If a full reroute is too large now, add a parity test suite first so the drift is visible.
4. 🟠 **Make `schema_version` a write-path invariant** — `crates/store/src/pages.rs:46`. A `stamp()` that runs after every `modify`/revert closure, so reconcile can never write new-shape bytes under an old stamp; the policy rests on this (#9).
5. 🟠 **Freeze append-only row identity** — `crates/store/src/store.rs:97`. Hash an explicitly destructured canonical encoding of `LogEntry`/`ThreadMessage`, with a frozen-value test, so a future field can't permanently kill cross-build sync (#19).
6. 🟠 **Decode entity references before the URL scheme test** — `web/src/lib/markdown.ts:43`. Decode numeric and named references, validate the decoded string, emit what was validated, and escape `&` on output; add the entity spellings to the refusal table (#57). Pair with a Playwright assertion on the CSP meta tag (#71).
7. 🟠 **Stop the web error paths erasing their own state** — `web/src/lib/components/Thread.svelte:107` (banner wiped by `reload()`, #60), `web/src/routes/page/[...path]/+page.svelte:63` (status fetch replaces the whole page, #62), `web/src/routes/+layout.svelte:25` (token gate stores on any non-401, #66).
8. 🟠 **Give `location_view` the degradation its siblings already have** — `crates/store/src/store.rs:915`. `get_or(..., empty)` for the four location docs, so a partial sibling set from an interrupted sync stops taking down `/api/queue` and every chat turn (#11).
9. 🟠 **Widen the queue degrade to `Err(_)`, not just NotFound** — `crates/assistant/src/views.rs:173`. A peer-synced `effort:"quick"` or `presence:"maybe"` currently 500s the queue and aborts the exchange; degrade the conversion or push tolerance into the hydrators (#85).
10. 🟠 **Add `IPAddressDeny` to the systemd unit** — `nix/module.nix:139`. The SSRF second line of defence is currently filesystem-only; deny the private ranges and the metadata address, then state in implementation.md exactly what it does and does not cover (#76).
11. 🟠 **Single-statement `log_rows()`** — `crates/store/src/store.rs:512`. Select uid + entry columns in one statement so a concurrent writer can't misalign the pairs and kill the sync round (#3).
12. 🟠 **One transaction for `append_log`** — `crates/store/src/store.rs:451`. Wrap the count, insert and promotion together so a failed promotion can't commit the cook and let the retry duplicate it (#4).
13. 🟠 **Cover the WebSocket in the SIGTERM drain** — `crates/server/src/lib.rs:227`. A `CancellationToken` select against `socket.recv()` so an interrupted sync still runs its post-session export (#46).
14. 🟠 **Reject a caller `id` in `shopping_add`** — `crates/assistant/src/tools.rs:1215`. Drop the field (address existing items via `shopping_update`) so a content-derived id can't reopen the cross-replica collision the mint closed (#81).
15. 🟠 **Trailing-dot hostname refusal** — `crates/assistant/src/fetch.rs:85`. Strip a single trailing `.` before matching; add `http://localhost./` to the policy table and a trailing-dot hop to the redirect test (#38).

## Resolved since this report

_2026-08-03 — single internal mutation API._ The CLI was a forked copy of the
tool layer; it now marshals its args and calls `tools::execute`, the one
internal API the HTTP surface (`/api/edit`) already uses. That one consolidation
closed **#33** and, by construction, its instances **#31** (HIGH — fridge ids
minted, not positional), **#32** (tier validated), **#35** (servings bounded at
CLI ingress), **#50** (equipment note preserved), **#51** (queue upsert patches).
Folded in: **#81** (`shopping_add` no longer trusts a caller id) and **#80** (the
convergence property now exercises and counts fridge portions; a CLI-binary
partition test and a CLI/tool parity test guard the surfaces). Fixes span jj
commits `zvsyutsk` (#81), `ztovxqmw` (#80), `kmkkkktp` (#33 et al.), `nuzswywm`
(#56 tests).

_2026-08-03 — `wss://` TLS._ **#52** (HIGH): `tokio-tungstenite` now carries
`rustls-tls-webpki-roots`, so the documented production sync (Caddy-terminated
TLS) connects instead of failing "TLS not compiled in" (jj `uzmvvkox`).

_2026-08-03 — reads are total by construction (tolerant-hydration cluster)._
Every read of a doc is now total, finishing the schema policy the campaign
started. **#19** froze append-only row identity behind an exhaustive destructure
plus a frozen-hash test (a new field is a compile error, not a silent desync).
**#9** made `schema_version` a write-path invariant via a `Stamped` trait
`modify` applies after the closure. **#11** gave `location_view` the missing-
sibling degradation its neighbours had. **#13/#85** typed the scalar doc fields
behind byte-preserving `repr` adaptors and made `to_core`/`to_view` infallible,
so a peer's out-of-vocabulary presence/effort/date/tier or a non-slug key
degrades instead of 500ing the queue — the `Corrupt` error path is gone at the
type level. `SCHEMA_VERSION` did not move; the frozen fixtures and export
byte-identity property are unchanged. Fixes span jj commits `rpmmuwyx` (#19),
`rqmwvqul` (#9), `nosymtvo`→`rqmwvqul` (#11), `totzuvrp` (#13/#85).

_2026-08-03 — canonicalize at the funnel, then validate (security cluster)._
Two textual checks were validating a non-canonical form while the effective form
differed. **#38**: `validate_url` now strips a trailing root dot before the
local-host suffix checks, so `localhost.` can't resolve past them. **#57**: the
markdown sanitizer decodes numeric character references before the scheme check
and escapes every `&` on output, so an entity-encoded `javascript:` can't re-form
in the `href` `{@html}` injects — no entity table to keep complete. Two
defense-in-depth gaps closed alongside: **#76** added `IPAddressDeny` for the
private/link-local/metadata ranges to the systemd unit (the second line the SSRF
deferral leans on), and **#71** added a Playwright assertion on the build's CSP
meta. Fixes span jj commits `sqmnzvpy` (#38), `vpszrysp` (#57), `oxmtxsqw` (#76),
`rzoqopol` (#71).

_2026-08-03 — one write unit, one source of truth (atomicity cluster)._
The transaction helper's own invariant — every multi-row write is one atomic
unit — is now actually held, and the two forked read paths are derived from
single sources. **#3**: `log_rows` selects uid and entry in one statement and
`log_entries` is derived from it, so the zip misalignment is unrepresentable.
**#4**: `append_log` prepares the promotion in memory, then commits count, cook
row and promotion change in one transaction — a failed promotion leaves no cook
row and no duplicating retry (also closes the CLI-beside-server occurrence-index
race). **#5**: prose reverts stage scalars and body splice on one loaded doc and
commit once — one history row, no half-reverted crash window. **#20**: the sync
round's transaction closure returns a `RoundDelta` folded into the outcome only
after commit, so a rolled-back round can no longer report (and git-commit) data
that never landed. **#34**: `read_page`/`list_pages` go through the new
path-addressed narrow read (`DocId::from_export_path` +
`Store::render_export_page`, covering log months and threads); byte agreement
with the full export is property-tested over every path. Fixes span jj commits
`kwzsoxsu` (#3), `lwvkrtlz` (#4), `lsytoxxw` (#5), `xoxosxsy` (#20), `qpyonpmo`
(#34).

_2026-08-03 — error state follows the operation (web error-state cluster)._
One root cause, two instances: a single page-level `error` with many writers and
an exclusive template branch. **#60**: `reload()` no longer clears the exchange's
error, so a server-side failure's banner survives the transcript reload that
used to erase it in the same tick. **#62**: the recipe-status side fetch degrades
to a hidden status row instead of replacing the rendered page with a banner.
**#66**: the token gate stores a candidate only on a genuine 2xx, distinguishing
"refused that token" from "server not answering". **#67**: the drafting box's
failed-turn cleanup filters on a structural `pending` marker (Thread.svelte's
pattern), not text equality, so a failed repeat no longer erases earlier
confirmed turns. Each carries an e2e regression. Fixes span jj commits
`wnyxtoky` (#60), `sztqsooy` (#62), `mqrumyvn` (#66), `zqzswowm` (#67).

Not yet addressed: #28 (unbounded thread history — needs a windowing policy
decision) and the local one-offs. Still deferred by decision: SSRF
resolve-and-pin (#45) — a public hostname resolving to loopback remains
reachable until it lands.

---

## Region index

| Region | 🔴 | 🟠 | ⚪ | Total |
|---|---:|---:|---:|---:|
| Domain core |  | 1 | 1 | 2 |
| Store — persistence, history, revert |  | 5 | 8 | 13 |
| Store — doc shapes, hydration, schema compat |  | 2 | 3 | 5 |
| Store — sync & threads |  | 1 | 2 | 3 |
| Assistant — Anthropic client |  | 2 | 2 | 4 |
| Assistant — turn, exchange, context |  | 1 | 1 | 2 |
| Assistant — tools & views |  | 3 | 3 | 6 |
| Assistant — fetch & recon |  | 1 | 7 | 8 |
| Server |  | 1 | 6 | 7 |
| CLI & remote | 2 | 4 | 5 | 11 |
| Web client |  | 5 | 6 | 11 |
| E2E suite & evals |  | 1 | 6 | 7 |
| Packaging (Nix) & tooling |  | 1 | 3 | 4 |
| Docs |  |  | 2 | 2 |
| **Total** | **2** | **28** | **55** | **85** |

Severity legend: 🔴 **HIGH** (fix first), 🟠 **MEDIUM**, ⚪ **LOW** (spec-drift, dead code, doc precision, test gaps).

---

## Domain core

**Files:** `crates/core/src/{types,readiness,coverage,rotation}.rs` (pure domain math), `crates/core/tests/properties.rs` (invariant properties)  
**Read first:** design doc → *The Queue* (readiness, lead time), *Steering*; CLAUDE.md → *Domain invariants are properties*, *Time is an input*  
**Key entry points:** `readiness::assess`, `coverage::coverage`, `rotation::recency`, `PantryItem::age_days`  
**Theme:** The math is right where the generators explore; the gaps are dead domain functions the model is left to reinvent above the seam.

### 🟠 **MEDIUM** · #0 · `PantryItem::age_days` — the domain's freshness-decay math — has zero production callers, so aging is left to the model to compute from raw ISO dates

**`crates/core/src/types.rs:239`** · _architecture_

`age_days(today)` is only referenced by its own unit test (types.rs:351). The pantry render (render.rs:213-236) emits a bare ISO `bought` column with no age, and `context::assemble` never injects a pantry view, yet the planning prompt tells the model to check "the pantry for what's aging" (context.rs:60). Deterministic date arithmetic that lives below the seam is being performed above it, where it cannot be tested.

- **Spec:** design.md:299 — "it checks the log (what's recent), the pantry (what's aging, what's stocked)"; CLAUDE.md — "If a piece of logic can't be tested without a model, it's on the wrong side of the seam."
- **Suggested fix:** Wire `age_days(ctx.today())` into a pantry read surface (an `age` column or aging summary), or record the deferral alongside #2 in implementation.md's "Known, scheduled after M7".

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/core/src/types.rs:236-244; crates/store/src/render.rs:213-236; crates/assistant/src/context.rs:60,96-114


Failure scenario: The model eyeballs ISO purchase dates against the clock line and misjudges what is aging; the domain function that would answer correctly is dead code that will drift.

</details>

### ⚪ **LOW** · #2 · `rotation::recency`, the computational basis of steering priority 1, is reachable only from a CLI debug subcommand

**`crates/core/src/rotation.rs:29`** · _architecture_

`recency` has exactly two callers outside its module: the import and the `rotation` debug subcommand in crates/cli/src/main.rs:872. Nothing in assistant/server/store references it, so the model does recency arithmetic itself over raw log markdown. Already ruled on by the user and recorded in implementation.md:471-479 as post-M7 work.

- **Spec:** design.md, Steering priority 1 — "Track recency across cuisine, protein, and format"
- **Prior:** deferred-by-decision
- **Suggested fix:** None wanted now. When scheduled: a rotation read tool returning `recency(&log, ctx.today(), window)` and/or a compact recency block in the planning context.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/core/src/rotation.rs:29-53; crates/cli/src/main.rs:872; crates/assistant/src/context.rs:60,96-114; docs/implementation.md:471-479


Failure scenario: The model judges rotation by eye from log markdown rather than from tested arithmetic.

</details>

---

## Store — persistence, history, revert

**Files:** `crates/store/src/store.rs` (SQLite + Automerge persistence, history, revert, export orchestration, id minting)  
**Read first:** implementation.md → *The page model*, *Revert semantics*, *Schema changes*  
**Key entry points:** `append_log`, `Store::revert`, `mint_id`, `export`, `log_rows`, `location_view`  
**Theme:** Several fixes from the last campaign left a sibling path on the old, unsafe pattern — two-statement reads, two-transaction writes, and a strict `get` where its neighbours now degrade.

### 🟠 **MEDIUM** · #3 · `log_rows()` builds uid→entry pairs by zipping two independent SELECTs, so a concurrent writer misaligns every pair after the insertion point

**`crates/store/src/store.rs:512`** · _bug_

`log_rows()` runs `SELECT uid FROM cook_log ORDER BY date, uid`, drops the statement, then calls `log_entries()` (a second ordered SELECT) and zips. Outside a transaction each statement gets its own WAL read snapshot, and `zip` silently truncates on length mismatch. The pairs go straight onto the sync wire. `thread_rows()` (store.rs:734-750) selects uid and payload in one statement and is immune; `log_rows()` drifted.

- **Spec:** implementation.md, Log-row identity: sync recomputes the content hash against the uid and rejects the round on mismatch; store.rs:146-151: "The design supports a `mise` CLI running beside the server on one file".
- **Suggested fix:** Make `log_rows()` a single statement selecting uid plus all entry columns, and express `log_entries()` as `log_rows()` with the uids dropped so the two orderings cannot diverge again.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every claim and each holds.

1. The code is exactly as described. `Store::log_rows()` (crates/store/src/store.rs:512-521) prepares `SELECT uid FROM cook_log ORDER BY date, uid`, collects the uids, `drop(stmt)`, then calls `self.log_entries()?` — a second, independent `SELECT date, kind, recipe, title, location, servings, verdict, tags FROM cook_log ORDER BY date, uid` (store.rs:524-528) — and combines them with `uids.into_iter().zip(...)`. Two statements, two implicit read snapshots, and `zip` truncates silently on a length mismatch rather than erroring.

2. There is no enclosing transaction at the call site. `sync.rs:327-332` calls `store.log_rows()?` after the ingest transaction has already been committed (sync.rs:307-319 closes its own `store.transaction(...)`), so the two SELECTs run as autocommit statements. In WAL mode each autocommit statement takes a fresh read snapshot, so a commit by another connection between them is visible to the second query but not the first.

3. The concurrency scenario is one the design explicitly supports, not a hypothetical: the `tune()` doc comment at store.rs:146-151 says "The design supports a `mise` CLI running beside the server on one file: WAL lets readers run under a writer...". A second process appending a log row mid-round is in scope.

4. The consequence is real on the wire. The zipped pairs are wrapped straight into `LogRow { uid, entry }` and shipped, and the peer's `ingest_log_row` (store.rs:1151-1159) recomputes `log_content_hash(e)` and returns `StoreError::Corrupt("log row uid ... does not match its content")` on any mismatch, aborting the round. An insertion that sorts before existing rows shifts every subsequent entry by one relative to its uid, so essentially all remaining pairs fail that check.

5. The contrast with `thread_rows()` is accurate: store.rs:734-750 selects `uid, thread, role, content, created` in a single statement and builds the pair from one row, so it cannot drift.

Note on blast radius (why not higher than medium): the receiver's hash check means the misalignment cannot silently persist wrong data — it fails the round loudly. The only way a mismatched pair slips through is when two entries share a content hash, i.e. identical entries, in which case the inserted row is still correct. So the impact is a failed/aborted sync session and a misleading "Corrupt" error, not corpus corruption. Medium is a fair call.

I could not construct a guard that prevents this; there is none.

</details>

### 🟠 **MEDIUM** · #4 · `append_log` commits the cook row and the draft→active promotion in two separate transactions, so a failed promotion commits the cook and the natural retry duplicates it

**`crates/store/src/store.rs:451`** · _bug_

`append_log` does a `COUNT(*)` for the occurrence index, `insert_log_row` (own transaction), then `modify::<RecipeDoc>` (another transaction). A failure in the third returns Err with the cook already committed, violating the function's own promise that no caller can log a cook and forget it. `Fail::from` maps the error to `Fail::Store`, aborting the exchange; on retry the `COUNT(*)` sees the new row and mints `-1`, writing a duplicate cook. Hydrate failures are reachable without disk faults because `Round.schema` is deliberately not a gate.

- **Spec:** store.rs:447-450 — "that rule lives here so no caller can log a cook and forget it"; implementation.md, Log-row identity — the `-n` suffix must mark a real repeat, not a retry.
- **Suggested fix:** Wrap count, insert and promotion in one `Store::transaction` closure, as `create_doc` and the sync round already are. Regression test: deny the `doc_changes` INSERT around `append_log`, assert `cook_log` is empty and the retry yields exactly one row.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced it and the structural claim holds exactly as written. `append_log` (store.rs:451-477) runs three separate units of work: a `COUNT(*)` read (457-461), `insert_log_row` which is its own `Store::transaction` (499-501), and then `modify::<RecipeDoc>` (469-473) whose write goes through `persist_change` → a *second* `Store::transaction` (269-276). Nothing wraps the pair. This directly contradicts the transaction helper's own stated invariant at store.rs:278-281 ("Every path that writes more than one row goes through here, so a kill, a full disk, or SQLITE_BUSY between statements cannot leave a doc row without its changes"), and contradicts the promotion promise in append_log's own doc comment at 446-448. `create_doc` (328-338) shows the correct one-transaction shape for exactly this reason.

The duplicate-on-retry mechanic is real: the uid is `format!("{scope}-{n}")` where `scope` is `log_content_hash(e)` + replica (456) and `n` is `COUNT(*)` of existing rows in that scope. `log_content_hash` (store.rs:97-101) is a pure hash of the serialized entry, so a retry of the identical entry produces the same scope, sees the now-committed row, mints `-1`, and `insert_log_row`'s idempotency guard (463-465, which only fires on an *identical* uid) does not fire. Two rows, permanently, in `cook_log` and in the export. And `Fail::from` (tools.rs:93-103) does map non-NotFound/Exists/Invalid/BadDocId errors to `Fail::Store`, which aborts the exchange (tools.rs:82), so the user-visible outcome is "it failed" followed by a natural re-send.

One correction to the finding's specific failure_scenario, which is why I'd not raise severity: both real callers hydrate the RecipeDoc *before* calling append_log — tools.rs:1173 (`store.get::<RecipeDoc>`) and cli/src/main.rs:847 — so a hydrate failure from a newer peer's shape would already have aborted the tool/command before any log row was written. The `Round.schema` non-gate at sync.rs:56-60 is correctly described, but that path is largely pre-empted here. The defect remains reachable via the ordinary failures the transaction comment itself enumerates: SQLITE_BUSY at the second `BEGIN IMMEDIATE` (another writer — the server sync path — holding the write lock), disk-full/IO error, or a plain process kill between the two commits. The kill case needs no error at all and yields both halves of the bug: an unpromoted draft plus a duplicating retry. Medium is the right severity.

</details>

### 🟠 **MEDIUM** · #11 · `location_view` hard-fails on a missing location sibling doc while `corpus()` and `render_page` were hardened to degrade, so a partial sibling set takes down readiness and assistant context

**`crates/store/src/store.rs:915`** · _bug_

`corpus()` and `render_page` use `get_or(..., Empty::empty)` with an explicit comment that partial location sets are reachable (a kill between the four per-doc creates, an interrupted first sync). `location_view` was left on strict `get` for all four kinds, and `active_view` — the entry point for context assembly and `/api/queue` — calls it. `add_location` creates the four docs before modifying the state doc, opening the same window from the other side.

- **Spec:** implementation.md → the `corpus()` remediation: a location's missing sibling doc degrades to empty rather than failing the read.
- **Suggested fix:** Use the same `get_or` degradation in `location_view` (`PantryDoc::empty`, `EquipmentDoc::empty`, `|| ShopsDoc::new(&[])`, `FridgeDoc::empty`) and note it in the doc comment. Regression test mirroring `corpus_tolerates_a_location_missing_a_sibling_doc`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and the asymmetry is real. `get_or` (crates/store/src/store.rs:805-810) degrades only `NotFound`, and both `corpus()` (store.rs:812-841) and `render_page` (store.rs:874-885) use it for all four location kinds, with explicit doc comments stating that partial sibling sets are reachable ("a kill between the four per-doc creates, an interrupted first sync") and must not erase the location. `location_view` (store.rs:915-928) was left on strict `self.get(...)` for pantry/equipment/shops/fridge, so any one missing sibling returns `StoreError::NotFound` for the whole view. `active_view` (store.rs:931-936) delegates to it, and it is the entry point for `assistant::views::queue_view` (crates/assistant/src/views.rs:88 — readiness/coverage/queue, hence chat context and `/api/queue`), the `/api/location` handler (crates/server/src/api.rs:106) and the proposal-annotation path (crates/server/src/api.rs:306). `add_location` (store.rs:409-435) does create the four docs one at a time via `create_location_docs` before the state-doc modify, and doc application is per-doc, so the described windows are consistent with what the codebase already documents as reachable. Two minor inaccuracies that do not change the defect: the error text in the failure scenario would be the missing doc id (e.g. "pantry/cottage") rather than "location cottage not found" (that message comes from the earlier strict `state.locations` lookup), and in the `add_location` crash window the state entry is missing too, so that variant fails at the meta lookup rather than the sibling `get`. The sync-interruption variant (state doc lands first, sibling doc not yet) hits exactly as described. Medium severity fits: it is a read-path availability regression under a partial-state window, not corruption, and the surrounding remediation already established the intended behavior.

</details>

### 🟠 **MEDIUM** · #15 · The export property never generates a value containing a newline or carriage return, so `esc`/`unesc` — the escape protecting the export's line structure — has zero coverage

**`crates/store/tests/support/mod.rs:455`** · _test-gap_

render.rs's module doc names newlines, pipes and backslashes as the escaped characters; only `|` and `\` are generated. `text()` draws from printable ASCII with no control characters, and every table cell and frontmatter value bottoms out in it. Multi-line bodies exist but are written raw after `## Method`, exercising no escaping. Interior newlines are reachable through the honest interface because `must_trim`/`opt_trim` strip only leading/trailing whitespace. Verified by hand that they round-trip today, so this is a coverage hole around the one property keeping the export parseable.

- **Spec:** crates/store/src/render.rs:6-8 — "the export stays the readable truth and the test-only parser can reverse it exactly"; CLAUDE.md, The export never lies.
- **Suggested fix:** Widen `text()` to emit interior `\n`/`\r`/`\r\n` (keeping the trailing trim); the property compares the parse against `store.corpus()`, so this widening is safe. Optionally assert a rendered file's line count is unchanged when a newline is injected into a cell.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/tests/support/mod.rs:455-476; crates/store/src/render.rs:6-8,21-50; crates/store/tests/export.rs:63; crates/assistant/src/tools.rs:130-141


Failure scenario: A future edit drops an `esc` call or adds a render site; `export_is_deterministic_and_complete` stays green because no generated value can break line structure.

</details>

### 🟠 **MEDIUM** · #19 · Append-only row identity is a hash of the live serde shape of `LogEntry`/`ThreadMessage` and sync hard-rejects any mismatch, so a future field on those structs permanently kills sync between builds

**`crates/store/src/store.rs:97`** · _architecture_

`log_content_hash`/`thread_content_hash` hash `serde_json::to_string` of the current build's struct, and that hash is the row's entire cross-replica identity; `ingest_log_row`/`ingest_thread_row` return `Corrupt` on mismatch, aborting the round transaction and the whole session. The doc side has a careful policy for this hazard (permanent tolerant hydrators, frozen fixtures, sync as a shape boundary that is not a gate); the append-only row shapes have no counterpart, no fixture pinning a known hash, and `Round.schema` is never consulted by verification.

- **Spec:** implementation.md, Schema changes: "Every doc-shape change ships a permanent tolerant hydrator"; "Sync is a shape boundary … It need not reject a mismatch."
- **Suggested fix:** Hash an explicitly enumerated, frozen canonical encoding (a destructuring match, so a new field is a compile error) rather than the live serde shape. Add a frozen-value test asserting the 16-hex prefix of a literal `LogEntry`/`ThreadMessage`. If the canonical form must ever change, version it into the uid and accept both prefixes forever.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/src/store.rs:97-108,1151-1176; crates/store/src/sync.rs:56-62,307-319; crates/server/src/lib.rs:266-270; crates/cli/src/remote.rs:127


Failure scenario: Add `#[serde(default)] photos: Vec<String>` to `LogEntry`. A phone on build N sends a row whose uid encodes the old hash; the desktop on N+1 recomputes a hash that now includes `"photos":[]`, returns Corrupt, and the entire round — including its doc changes — rolls back. Every retry fails identically, in both directions, forever, and a third device can never complete its first sync.

</details>

### ⚪ **LOW** · #5 · `Store::revert` on a recipe or technique writes two history changes in two transactions, not the single forward change the spec describes

**`crates/store/src/store.rs:585`** · _bug_

The prose arms restore scalars through `modify` then the body through `update_body`, each opening its own transaction via `persist_change`. Verified: one revert yields two identical `ui: revert` history entries. It is also non-atomic — a failure between them leaves the target revision's metadata with the current body, a state that never existed in history. The structured arms (`revert_plain`) are single-change and correct.

- **Spec:** implementation.md, Revert semantics — "restores the page to its state as of that change, recorded as a *new forward change*"
- **Suggested fix:** Give prose reverts a single-transaction path: apply field assignments and the body splice to one loaded `AutoCommit`, commit once, persist once. Extend the recipe revert property to assert history grows by exactly one.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and it matches the finding. `Store::revert` (crates/store/src/store.rs:585) dispatches structured docs to `revert_plain` (:662-672), which does a single `modify` → single commit → single `persist_change`. The two prose arms are different: `DocId::Recipe` (:617-647) calls `self.modify::<RecipeDoc>(...)` for the scalars and then `self.update_body(id, &old_body, ...)`; `DocId::Technique` (:648-658) does the same. `modify` (:343-359) ends with `doc.commit_with(stamp(provenance, at))` followed by `persist_change`, and `update_body` (:370-407) independently ends with its own `commit_with` + `persist_change`. `persist_change` (:269-276) opens its own `self.transaction(...)` per call. So when a revert restores both a changed scalar and a changed body, two Automerge changes are committed, each carrying the same provenance message and timestamp, and each written in a separate SQLite transaction — two identical history rows rather than the single forward change the spec describes (docs/implementation.md:210-215: "restores the page to its state as of that change, recorded as a *new forward change* — history only grows"). The non-atomicity claim also holds: `modify` persists first, so a crash between the two transactions leaves the doc with the target revision's metadata and the current body — a combination that never existed in history and that a later revert cannot name as a single hash. The existing recipe revert property (crates/store/tests/store_behavior.rs:392-425) only asserts the hydrated value equals the snapshot; it asserts "one edit is at most one change" for the *edit* loop, never for the revert, so the duplicate-change behavior is untested. Severity low is fair — the end state is correct on the happy path, so the impact is a cosmetic double row in the history feed plus a narrow crash window.

</details>

### ⚪ **LOW** · #6 · Superseded `doc_snapshots` rows are never pruned, so the backed-up database grows quadratically in change count for data nothing reads

**`crates/store/src/store.rs:1093`** · _architecture_

`append_changes` inserts a full `doc.save()` every 64 changes, each embedding the doc's entire history to that point. `load_doc_rows` only ever reads `ORDER BY upto_seq DESC LIMIT 1`; the only DELETE is inside the one-shot v3→v4 repair. Measured: 500 modifications to one pantry doc produce 7 snapshots totalling 58,530 bytes of which only 14,611 is live.

- **Spec:** implementation.md, On-disk layout — "`mise.db` is what gets backed up."
- **Suggested fix:** In the same transaction as the insert, `DELETE FROM doc_snapshots WHERE doc_id = ?1 AND upto_seq < ?2` (optionally keeping one previous snapshot with an explicit bound).

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/src/store.rs:1093-1103,1011-1023,1228-1248,30-32


Failure scenario: A pantry doc with 5,000 changes carries ~78 snapshot rows summing to 5-6 MB while the live snapshot is ~150 KB; across a dozen docs the backup holds tens of megabytes nothing will ever read, with no reclamation.

</details>

### ⚪ **LOW** · #7 · `mint_id`'s uniqueness rests on a counter in the un-synced `meta` table, so restoring `mise.db` from a backup re-issues collection keys already live on peers

**`crates/store/src/store.rs:487`** · _bug_

`mint_id` returns `<prefix>-<replica>-<seq>` with `seq` from `meta.id_seq` and the replica id also in `meta`. `meta` is not part of the sync protocol, so the counter's only defence against reuse is that the file is never rewound — but `mise.db` is explicitly the backed-up artefact, and a restore rewinds `id_seq` while keeping the replica id.

- **Spec:** store.rs:479-486 — "The counter never reuses"; implementation.md — "`mise.db` is what gets backed up."
- **Suggested fix:** Either raise `id_seq` on open past the highest `<replica>-<n>` suffix present in ShoppingDoc/FridgeDoc, or mint the suffix from `getrandom` as the replica id is; record the choice in implementation.md → Schema changes.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the mechanism end to end and found no guard.

1. `mint_id` (crates/store/src/store.rs:487-496) builds `<prefix>-<replica>-<seq>` where `seq` comes from an upsert on `meta.id_seq` and `replica` from `meta.replica_id` (`ensure_replica_id`, store.rs:161-174). Both live in the `meta` table.
2. `meta` is purely local: `migrate_v4_to_v5` (store.rs:1205-1219) creates it, and nothing in the sync path touches it. `Peer::start`/`initial_round`/`handle` (crates/store/src/sync.rs:199-224, 265-319) exchange only Automerge doc messages plus log/thread uids and rows — no meta rows, no id-watermark. So a peer never learns another replica's high-water seq, and — the point of the finding — a restored replica never learns its own.
3. Nothing on `open` (store.rs:211-219 → `migrate` → `ensure_replica_id`) raises `id_seq` past ids already present in ShoppingDoc/FridgeDoc. `ensure_replica_id` deliberately *preserves* an existing replica id, so a restore keeps the identity while rewinding the counter — exactly the combination the finding describes.
4. The collision is a silent overwrite, not a detected error: `shopping_add` does `d.items.insert(assigned.clone(), ...)` (crates/assistant/src/tools.rs:1216-1230) and `fridge_add` does `portions.insert(...)` (tools.rs:1079-1090); neither checks whether the minted key is already occupied. So re-minting `s-<replica>-41` after the restored replica has re-synced "eggs" at that key overwrites it with "tofu" — the very failure the doc comment at store.rs:479-486 says the counter exists to prevent.

The only thing I could not verify is the operational premise, and the docs supply it: docs/implementation.md:556 says "`mise.db` is what gets backed up." A restore of that file is therefore an expected operation, and it rewinds `id_seq`.

Severity low is right: it needs a backup restore plus a prior sync of ids above the restore point, and the damage is bounded to items minted after the backup. Left unchanged.

</details>

### ⚪ **LOW** · #8 · `migrate`'s doc comment was left attached to `load_doc_rows`, so one function documents the wrong thing and the other is undocumented

**`crates/store/src/store.rs:1006`** · _quality_

Lines 1006-1007 describe `migrate` ("Bring an existing database up to the current schema...") but sit in the same `///` block as `load_doc_rows`'s real description. `fn migrate` at store.rs:1179 has no doc comment at all, leaving the trickiest code in the file — a four-step migration ladder with hand-rolled transactions — unoriented.

- **Spec:** CLAUDE.md, Documents — docs and code describe the same system.
- **Suggested fix:** Move the first two lines back above `fn migrate` at store.rs:1179.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/src/store.rs:1006-1011,1179


Failure scenario: n/a — documentation defect, no runtime behaviour.

</details>

### ⚪ **LOW** · #16 · The export completeness property still only generates already-normalized corpora, so the render/parse pair is unverified on untrimmed strings and empty options in structured pages

**`crates/store/tests/support/mod.rs:460`** · _test-gap_

The ingress half of the prior fix landed (thread and log normalization); the generator half did not — `text()` still trims and `opt_text()` filters empties. Structured page docs arrive over sync as opaque Automerge changes applied wholesale, so there is no ingress point to normalize them: a peer can carry `PantryItemDoc.note = Some("  x  ")` (parsed back trimmed) or `DishRefDoc.title = ""` (rendered as an all-empty row that `parse_queue` drops).

- **Spec:** CLAUDE.md, The export never lies: "export → parse → structural compare against store state for structured pages."
- **Prior:** still-open
- **Suggested fix:** Either document the normalized subset explicitly in support/mod.rs and render.rs, naming the two lossy spots, or make the subset unrepresentable with a `Trimmed(String)` newtype with a tolerant Hydrate (mirroring `repr::status`) and then widen the generators.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/tests/support/mod.rs:99-120,170-175,455-471; crates/store/src/render.rs:139-151; crates/store/src/store.rs:812


Failure scenario: A peer-written empty dish title renders an all-empty queue row that the test parser reads as "entry with no dishes", dropping the dish, and the property cannot see it.

</details>

### ⚪ **LOW** · #17 · The export's stale-file sweep follows symlinks, so a symlink inside `export/` makes it delete files outside the export tree or recurse until the stack overflows

**`crates/store/src/store.rs:1360`** · _bug_

`collect_files` descends on `path.is_dir()`, which follows symlinks, so anything reachable through a symlinked directory is collected with a path lexically under `base`; `export()` then `remove_file`s every collected path not in the render map, unlinking the target. `remove_empty_dirs` has the same issue and errors with ENOTDIR on a symlink. A symlink cycle recurses without a depth bound. Guarded by the threat model, but the failure mode is destroying data that was never the store's.

- **Spec:** implementation.md, On-disk layout — the sweep's mandate is the export tree, not whatever it links to.
- **Suggested fix:** Use `entry.file_type()?` (which does not traverse) in both functions; treat a symlink as a stale non-directory entry and `remove_file` the link. Regression test: symlink to a tempdir inside `export/`, run `export()`, assert the target survives and the link is pruned.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and the finding matches it exactly.

`collect_files` (crates/store/src/store.rs:1353-1372) branches on `path.is_dir()`, which is `Path::is_dir` — it calls `fs::metadata`, which *follows* symlinks. So a symlinked directory inside the export tree is descended into, and every file underneath is pushed with a path lexically relative to `base` (the `strip_prefix(base)` succeeds because `entry.path()` is built by joining, not canonicalized).

`export()` (store.rs:964-970) then does `std::fs::remove_file(dir.join(&rel))` for every collected rel path not present in the render map. Since the render map only ever contains the rendered export files, everything reached through the symlink is "stale". `dir.join("notes/taxes.pdf")` resolves the `notes` symlink at unlink time, so this deletes the real target file outside the export tree — not the link. The failure_scenario is accurate.

The cycle case is also real: with `export/loop -> export/`, `path.is_dir()` stays true at every level and `collect_files` recurses with no depth bound.

`remove_empty_dirs` (store.rs:1374-1394) has the same `path.is_dir()` traversal; it would attempt `remove_dir` on a symlink path, which fails with ENOTDIR, aborting the export after the SQLite mutation already committed.

No guard exists anywhere: `grep -rn "symlink\|file_type()\|is_symlink" crates --include="*.rs"` returns nothing, so neither walker ever consults a non-traversing file type.

Severity "low" is right — the memory'd threat model treats local filesystem state as trusted, and nothing in the app creates these symlinks itself; it takes an external actor (a sync tool, a user's own `ln -s`) to set up. But the blast radius when it does happen is destroying files the store never owned, so the suggested fix (`entry.file_type()?` in both functions, treating a symlink as a stale non-directory entry) is worth taking.

</details>

### ⚪ **LOW** · #23 · Uid verification checks only the content-hash prefix and a non-empty suffix, so a peer can inject unlimited duplicates of one entry under fabricated suffixes and rows in undefined uid forms are accepted permanently

**`crates/store/src/store.rs:1151`** · _security_

`ingest_log_row`/`ingest_thread_row` accept any uid of the form `<sha256(content)[..16]>-<anything non-empty>`; the spec defines `<hash>-<replica-id>-<n>` (with the legacy two-part form grandfathered) but the suffix is never parsed and no test pins its shape. It interacts with `append_log`'s `COUNT(*) WHERE uid LIKE '<hash>-<replica>-%'`: because the replica id travels in every uid a device ships, a peer can occupy indices in our replica's space and make a later local append fail with `Corrupt("log uid ... already taken")`.

- **Spec:** implementation.md, Log-row identity: "uid = `sha256(entry)[..16]-<replica-id>-<n>`".
- **Suggested fix:** Parse the suffix: accept `<hex-replica>-<decimal-n>` or the legacy bare `<decimal-n>` and reject anything else; add a test that both accepted forms round-trip and a third form is refused.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code says exactly what the finding claims. `ingest_log_row` (store.rs:1151-1159) builds `prefix = format!("{}-", log_content_hash(e))` and accepts the uid if `uid.strip_prefix(&prefix)` yields any non-empty suffix — no parsing of `<replica-id>-<n>` at all; `ingest_thread_row` (1165-1176) is structurally identical. These are the only ingress checks: sync.rs:309/314 calls them directly from the round transaction with the wire-supplied `row.uid`, and there is no earlier uid validation in `Peer::handle`. So `<hash>-anything` passes, and because `insert_log_row` is `INSERT OR IGNORE` on a uid PK, N distinct fabricated suffixes over identical content yield N persisted rows with no delete path.

The interaction with `append_log` (store.rs:451-465) also holds: `scope = "<hash>-<replica>"`, `SELECT COUNT(*) ... WHERE uid LIKE scope || '-%'`, `uid = "{scope}-{n}"`. Injected rows in *our* replica's namespace inflate the count, so once an injected index equals the next count (e.g. own rows -0,-1,-2 plus injected -3 and -5 gives count 5 → uid `<hash>-<replica>-5`), `insert_log_row` returns false and the local append fails with `Corrupt("log uid ... already taken")` — a permanent failure for that content on that replica.

The test at crates/store/tests/sync.rs:247-260 only pins the content-hash half (`0000000000000000-0` rejected); nothing pins the suffix shape.

Caveats that argue for keeping severity at "low", not raising it: the loose check is deliberate and documented — implementation.md:94-95 says "Sync verifies every incoming row by recomputing the content hash against the uid ... pre-replica two-part uids remain valid forever under the same check", and store.rs:1145-1150 / migrate_v4_to_v5's comment (store.rs:1205-1207) say the same. So this is a hardening gap rather than a code/doc disagreement, and under the project's threat model sync peers are the user's own devices. The suggested fix (accept `<hex-replica>-<decimal-n>` or legacy bare `<decimal-n>`) remains compatible with the grandfathering the docs promise.

</details>

### ⚪ **LOW** · #82 · `ensure_replica_id` does an unsynchronised read-then-insert, so two processes opening a freshly-migrated corpus at the same moment race and one fails `Store::open` with a raw UNIQUE-constraint error

**`crates/store/src/store.rs:162`** · _bug_

It runs `SELECT value FROM meta WHERE key='replica_id'` and on None generates 4 random bytes and plainly INSERTs, with no transaction around the pair and `meta.key` a PRIMARY KEY. The busy timeout serialises the writes but cannot undo the stale read, so the loser gets `SQLITE_CONSTRAINT_PRIMARYKEY` propagated as an opaque `StoreError::Sqlite`. The window is one per corpus lifetime (the first open after the v4→v5 migration creates the empty `meta` table), which is why it is low — but the deployment explicitly supports a CLI running beside the server on one file.

- **Spec:** store.rs:146-151: "The design supports a `mise` CLI running beside the server on one file."
- **Suggested fix:** Make it one atomic statement — `INSERT ... ON CONFLICT(key) DO NOTHING` then re-SELECT — or use the UPSERT-with-RETURNING shape `mint_id` already uses at :488-494, which is race-free by construction.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code is exactly as described. `ensure_replica_id` (store.rs:162-174) does a bare `SELECT value FROM meta WHERE key = 'replica_id'` with `.optional()?`, and on `None` mints 4 random bytes and issues a standalone `conn.execute("INSERT INTO meta (key, value) VALUES ('replica_id', ?1)")`. There is no surrounding transaction, no `ON CONFLICT`, and no retry — and `meta.key` is declared `TEXT PRIMARY KEY` in `migrate_v4_to_v5` (store.rs:1211-1214), so a losing writer gets SQLITE_CONSTRAINT_PRIMARYKEY surfaced as an opaque `StoreError::Sqlite` out of `Store::open` (store.rs:216-219). I checked for a guard the finding might have missed and found none: `SCHEMA` (store.rs:34) never pre-seeds a `replica_id` row (grep shows the only three references to the key are the SELECT, the INSERT, and the fn name), so a freshly migrated or freshly created corpus genuinely has an empty `meta`. The `busy_timeout` set in `tune` (store.rs:156) only serialises the writes; it cannot invalidate the stale read, and rusqlite does not retry a constraint failure. The contrast the finding draws with `mint_id` (store.rs:487-495), which uses the race-free `INSERT ... ON CONFLICT(key) DO UPDATE ... RETURNING` shape, is accurate and is the right fix template. The window is genuinely narrow — between one process committing the v4→v5 migration (or `create_bare`'s `execute_batch(SCHEMA)`) and its own subsequent replica insert — and occurs at most once per corpus lifetime, so "low" is the right severity; the doc comment at store.rs:146-151 does explicitly support a CLI running beside the server on one file, so the scenario is in-scope rather than hypothetical.

</details>

---

## Store — doc shapes, hydration, schema compat

**Files:** `crates/store/src/{pages,docid}.rs`, `crates/store/tests/schema_compat.rs`, `tests/fixtures/schema-v1/`  
**Read first:** implementation.md → *Schema changes* (permanent tolerant hydrators, `schema_version_at`, frozen fixture bytes, typed fields same bytes)  
**Key entry points:** `Store::modify` reconcile, `schema_version_at`, `LocationDocs::to_view`, the `repr` adaptors  
**Theme:** The schema-migration machinery the whole policy rests on is not yet load-bearing: the stamp is never re-written, the compat harness covers one version, and some hydrators still hard-fail.

### 🟠 **MEDIUM** · #9 · Nothing re-stamps `schema_version` on write, so the field records the version a doc was created at, not the shape its bytes are in

**`crates/store/src/pages.rs:46`** · _bug_

`SCHEMA_VERSION` is written only by constructors and two literal `RecipeDoc` sites. Every later write goes through `Store::modify`, which reconciles today's shape while preserving the stored stamp verbatim. `Store::revert` is worse: both prose arms restore the historical stamp and `revert_plain` assigns the whole historical value, rewriting the stamp downward over current-shape bytes. The stale value also renders into export frontmatter on every page. No divergence today because SCHEMA_VERSION == 1, but this is the exact machinery the schema policy rests on.

- **Spec:** implementation.md → Schema changes: "`pages::schema_version_at` is its reader, for the cases presence cannot answer — a field whose meaning changed while its name and type did not."
- **Suggested fix:** Make the stamp a write-path invariant: a small `Stamped` trait whose `stamp()` runs after the closure in `modify` and in the revert paths, so reconcile can never write new-shape bytes under an old stamp. Test create→modify→`schema_version_at == SCHEMA_VERSION`, and the same after a revert.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every claim and they hold.

1. `SCHEMA_VERSION` (crates/store/src/pages.rs:33) is referenced in-crate only by the `*Doc::new()/empty()` constructors and by the sync handshake (`crates/store/src/sync.rs:176,217,353`). Outside the store crate it appears only at two literal construction sites (`crates/assistant/src/tools.rs:799`, `crates/cli/src/main.rs:629`) plus a remote handshake. Nothing on a write path re-stamps it.

2. `Store::modify` (crates/store/src/store.rs:343-359) is `hydrate` -> closure -> `reconcile` with plain `autosurgeon::{hydrate, reconcile}` (imported at store.rs:17 — no wrapper that injects a stamp). `schema_version` is an ordinary derived field on every doc struct (e.g. `StateDoc.schema_version`, pages.rs:71; `QueueDoc`, pages.rs:111), so the hydrated stamp is carried straight back out through reconcile while the rest of the object is rewritten in today's shape. So after a v2 build edits a v1-created doc, the bytes are v2-shaped and the stamp still reads 1.

3. The revert paths are as described. `revert_plain` (store.rs:662-672) does `*v = value` with the whole historical value, stamp included; the two prose arms explicitly assign `r.schema_version = schema_version` (store.rs:634) and `t.schema_version = schema_version` (store.rs:653) from the destructured historical doc — and both then write through `modify`/`update_body`, i.e. current-shape bytes under a historical stamp.

4. The stale value does reach the export: `render.rs` emits `schema-version` frontmatter straight from the field on every page kind (render.rs:115,131,159,184,215,241,254,267,306,348 and the kv_page calls at render.rs:90-91).

5. The spec rule is quoted correctly: docs/implementation.md:424-431 gives `schema_version` the job of answering "a field whose meaning changed while its name and type did not", which is precisely the question a never-re-stamped field cannot answer for a doc that has been rewritten since creation. The doc does hedge ("tolerant hydrators prefer presence, because a merged doc carries the new field beside an old stamp"), which is why the stamp is only relied on for the meaning-change case — the one case the finding's failure scenario exercises. Double-application of a v1->v2 meaning conversion is a genuine consequence of that.

No divergence is observable today because `SCHEMA_VERSION == 1` and there is exactly one shape version, so this is latent rather than live — `medium` is a fair rating for machinery the stated schema policy rests on. Worth noting for the fix discussion (not a refutation): unconditional re-stamping in `modify` interacts with sync, since an older peer editing the same doc would stamp downward again; the invariant probably wants to be monotone (`max(stored, SCHEMA_VERSION)`) rather than a plain assignment.

</details>

### 🟠 **MEDIUM** · #12 · The schema-compat harness is hard-wired to one version directory, so the "every past version stays covered forever" promise its own module doc makes has no mechanism behind it

**`crates/store/tests/schema_compat.rs:44`** · _test-gap_

`const VERSION_DIR = "schema-v1"` feeds the single `fixture_dir()`/`fixture_path`, and `manifest()` is a list of doc kinds with no version axis. The natural mechanical edit when v2 lands — flip the const and update `v1_values()` — leaves the v1 `.change` files orphaned on disk with the suite green, so the permanent v1 tolerant hydrator could be deleted unnoticed. This file is the only enforcement of the schema policy.

- **Spec:** implementation.md → Schema changes: "A version's bytes are deleted only when the hydrator that reads them is, which is never."
- **Suggested fix:** Make the version a dimension: discover or list `schema-v*` directories and loop the hydrate/render/revert assertions over all of them with per-version manifests; assert the discovered set covers 1..=SCHEMA_VERSION so deleting a directory fails loudly.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/tests/schema_compat.rs:22-25,44,62-103,105-112,264-266


Failure scenario: v2 ships, VERSION_DIR is flipped, the v1 fixtures become unread, and a later refactor drops the v1 hydrator with the whole suite still green.

</details>

### ⚪ **LOW** · #10 · `RecipeDoc`/`TechniqueDoc` hand-enumerate their fields in `PartialEq`, so a newly added field is invisible to the CRDT convergence property and every equality assertion

**`crates/store/src/pages.rs:403`** · _bug_

Both prose docs need a manual `PartialEq` (autosurgeon::Text has none) and spell the field list out by hand. `Store::revert`'s prose arms were changed to destructure precisely so a new field is a compile error; `PartialEq` is the other hand-enumeration and was missed. It is load-bearing: `tests/convergence.rs` asserts `prop_assert_eq!(a.snapshot(), b.snapshot())` through this `eq`, and `CorpusState`'s derived `PartialEq` inherits it.

- **Spec:** implementation.md → Schema changes: "Wherever a doc's fields are enumerated by hand … the hydrated value is destructured, so a newly added field is a compile error rather than a silently skipped one."
- **Suggested fix:** Destructure both sides in `eq` with a `let RecipeDoc { .. } = self;` pattern so adding a field fails to compile. Same for `TechniqueDoc`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced it and the code says exactly what the finding claims. `RecipeDoc` (crates/store/src/pages.rs:379-401) and `TechniqueDoc` (:459-465) derive `Reconcile, Hydrate` but not `PartialEq` (they hold `autosurgeon::Text`), and both get a hand-written `impl PartialEq` that spells out `self.x == other.x` field by field (:403-417 and :467-474) rather than destructuring. Adding a field to the struct compiles fine and is silently omitted from equality.

It is load-bearing exactly as described: `crates/store/tests/convergence.rs:111-119` builds `Snapshot` from hydrated docs including the `RecipeDoc`, and the convergence property asserts through it at :322 (commutativity), :327 (full sync) and :332 (idempotence) via `prop_assert_eq!`; `CorpusState`'s derived `PartialEq` also flows through these impls (`corpus()` at :121-144 embeds the recipe). So a field skipped in `eq` would make genuinely divergent replicas compare equal.

The cited spec rule is real: docs/implementation.md:454-458 says "Wherever a doc's fields are enumerated by hand — `Store::revert`'s prose arms — the hydrated value is destructured, so a newly added field is a compile error", and `Store::revert` does destructure (`let RecipeDoc {` at crates/store/src/store.rs:619). These `PartialEq` impls are the second hand-enumeration site and were not given the same treatment.

Severity note: I verified that today both impls do cover every field (RecipeDoc: all 11; TechniqueDoc: all 4), so there is no live incorrect comparison — the defect is purely a missing compile-time guard against a future field. That makes it a latent robustness/spec-compliance gap rather than a current bug, so I'd call it low rather than medium. The suggested fix (destructure both sides) is correct and cheap.

</details>

### ⚪ **LOW** · #13 · `LocationDocs::to_view`/`PantryItemDoc::to_core` hard-fail on out-of-vocabulary values the `repr` adaptors were introduced to degrade, so one bad string from a peer kills every readiness read for that location

**`crates/store/src/pages.rs:498`** · _architecture_

pages.rs:301 states the tolerance principle and the `repr` adaptors honour it for `RecipeDoc.status`, equipment lists and ingredient pantry links. The location side is still stringly and fatal: every `EquipmentDoc.items` key and `ShopsDoc` tier id goes through `parse_slug`, and `to_core` hard-errors on an unparseable `presence` or `bought` date, all collected with `?`. The same equipment slug is drop-tolerant in a recipe and fatal as a map key.

- **Spec:** implementation.md → Schema changes, Typed doc fields, same bytes: "hydrate tolerantly … degraded, never a dead read"
- **Suggested fix:** Type the fields (`presence: Presence`, tier/equipment keys as `Slug`) behind `repr`-style adaptors that keep the v1 bytes and degrade on the way in, or at minimum skip the unreadable entry rather than failing the whole view; cover with a peer-written bad `presence` test.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/src/pages.rs:203-207,301,498-540


Failure scenario: A peer syncs equipment key `"Stand Mixer"` or `presence: "maybe"`. Hydration and export succeed, but `location_view(home)` returns `Corrupt`, so `queue_status`, `/api/queue` and context assembly fail for that location until someone edits the raw value out.

</details>

### ⚪ **LOW** · #14 · `schema_version_at` saturates an oversized `Uint` stamp to `u32::MAX` but an oversized `Int` stamp to `0`, so the same out-of-range version reads as newer-than-everything or older-than-everything depending on the Automerge scalar type

**`crates/store/src/pages.rs:50`** · _bug_

`:49` maps `Uint(n)` with `unwrap_or(u32::MAX)`; `:50` maps `Int(n)` with `unwrap_or(0)`, which is right for negatives but wrong above `u32::MAX` — it reports the value the doc comment defines as "older than anything", the unsafe end for an over-range stamp. Automerge writers choose Int or Uint freely, so which branch runs is not under this build's control. Latent (no caller branches yet) but it is a divergence inside the function whose whole purpose is being the safe reader.

- **Spec:** pages.rs:44-45 — "Missing or unreadable reads as `0`: older than anything that ever stamped a version, which is the safe end of every comparison."
- **Suggested fix:** Split the `Int` case by sign: negative → 0, otherwise `unwrap_or(u32::MAX)`. Extend the `schema_version_at` assertions with a hostile-stamp table (negative, u64::MAX, a string, a map).

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code reads exactly as the finding describes. `crates/store/src/pages.rs:48-53`:

    automerge::ScalarValue::Uint(n) => u32::try_from(*n).unwrap_or(u32::MAX),
    automerge::ScalarValue::Int(n)  => u32::try_from(*n).unwrap_or(0),

For a stamp above `u32::MAX` the `try_from` fails in both arms, but the fallbacks are opposite: `Uint` saturates to `u32::MAX` ("newer than everything") while `Int` falls to `0` ("older than everything"). No guard elsewhere normalizes the scalar type before this point — this is the only reader (grep for `schema_version_at` finds only this definition, two test call sites in `crates/store/tests/schema_compat.rs:182,188`, and a docs mention at `docs/implementation.md:427`). Automerge does let a writer store the same numeric value as either `Int` or `Uint`, so which arm runs is not under this build's control, and the same over-range value therefore reads as two maximally different versions. The doc comment at `pages.rs:44-45` declares one intended fallback ("Missing or unreadable reads as `0`"), which the `Uint` arm contradicts, so the inconsistency is real regardless of which end one considers "safe".

Caveat on framing, not on the defect: since the doc comment names `0` as the *safe* end, one could argue the `Int` arm is the compliant one and the `Uint` arm is the bug — the finding's failure_scenario picks the opposite reading. Either way, a single function whose stated purpose is being the tolerant reader has two contradictory out-of-range behaviors. Severity `low` is right: no production caller branches on the value yet (only tests), so this is latent.

</details>

---

## Store — sync & threads

**Files:** `crates/store/src/{sync,threads}.rs`, `crates/store/tests/{convergence,sync}.rs`  
**Read first:** implementation.md → *Sync protocol* (M2), *Threads are log-shaped*, *Schema changes* (replica-scoped uids, sync is a shape boundary)  
**Key entry points:** `Peer`, `ingest_log_row`/`ingest_thread_row`, `log_content_hash`  
**Theme:** Data no longer silently vanishes on the round, but identity now rests on the *live serde shape* of the row structs, and a mismatch is a hard, permanent sync-killer.

### 🟠 **MEDIUM** · #80 · No convergence test allocates ids through a real entry point, and the reconvergence property has no fridge op at all — which is why the surviving positional allocator passes a green suite

**`crates/store/tests/sync.rs:345`** · _test-gap_

The prior fix's stated remedy was "extend the convergence property to allocate ids through the real tool path and assert item *count* is preserved". Only half landed: `tests/sync.rs`'s `Op` enum is `Pantry | Queue | Log | Thread | Shopping` with no `Fridge` variant, so fridge portion ids are never exercised across a partition anywhere; and `Op::Shopping` calls `store.mint_id("s")` directly rather than going through `tools::execute`, so it validates `mint_id` itself rather than its callers. `tests/convergence.rs` is worse for this purpose — its `ShoppingAdd`/`FridgeAdd` ops use fixed key spaces (`format!("s{}", k % 6)`), so a collision there is the point of the test and can never fail.

- **Spec:** CLAUDE.md → CRDT convergence: "Generate random operation sequences and apply them under seeded interleavings and partition/merge scenarios"; prior review #75's remedy.
- **Prior:** still-open
- **Suggested fix:** Add a `Fridge` variant to `tests/sync.rs`'s `Op` and extend the count assertions in `divergent_stores_reconverge` and `a_session_cut_at_any_point_loses_nothing` to portions. More importantly, make at least one arm allocate through a real entry point (`tools::execute` for the web/chat surface, plus a CLI-level test) so a front door minting its own ids cannot pass. A structural alternative: have FridgeDoc/ShoppingDoc expose an `insert_new(&mut Store, value)` that mints internally so no caller can supply a key.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/tests/sync.rs:345,430-440; crates/store/tests/convergence.rs:194,239


Failure scenario: `cargo test -p mise-store` and `-p mise-cli` both pass today while `mise fridge add` on two partitioned replicas destroys a portion on merge; adding a `Fridge` op that allocates the way the CLI does, plus a portion-count assertion, fails immediately.

</details>

### ⚪ **LOW** · #20 · `SyncOutcome` counters are mutated inside the round's transaction closure and survive its rollback, so a failed round is reported — and git-committed — as data that landed

**`crates/store/src/sync.rs:309`** · _bug_

`log_added`, `threads_added` and `docs_updated` are incremented inside `store.transaction(|tx| ...)` but live on `Peer`, so a rollback leaves the increments. Both drivers consume the outcome on the failure path: `remote::sync` returns the outcome regardless of the result and `main.rs:416-424` exports with `describe(&outcome)` as the commit message before propagating the error; the server's post-session export gate is driven by the same counters. `Peer::commit` also advances `dp.baseline` inside the closure, harmless only because the session is always discarded after an error — an invariant nothing enforces.

- **Spec:** implementation.md, Sync protocol: "one SQLite transaction per round, so a failure mid-round persists all of it or none of it".
- **Suggested fix:** Accumulate the round's counts (and new baselines) in locals returned from the closure and fold them into `self.outcome` only after `store.transaction(...)` returns Ok. Regression test asserting `pb.outcome().is_empty()` after a rejected round.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code says exactly what the finding claims.

1. `crates/store/src/sync.rs:307-319`: `self.outcome.log_added += 1` and `self.outcome.threads_added += 1` are incremented inside the `store.transaction(|tx| ...)` closure, and `self.commit(tx)` (lines 249-263) inserts into `self.outcome.docs_updated` and advances `dp.baseline = dp.doc.get_heads()` inside the same closure. All of that state lives on `Peer`, not on the transaction.

2. `crates/store/src/store.rs:282-298`: `let out = f(&stx)?; stx.tx.commit()?;` — the `?` on `f(&stx)` returns early, dropping `StoreTx` and rolling back the rusqlite transaction. Nothing rewinds the `Peer`-side mutations, so any error after the first successful `ingest_log_row` (a later row's SQL error, a `DocId::parse` failure or `append_changes` error inside `commit`) leaves counters reflecting writes that were rolled back. This directly contradicts the "persists all of it or none of it" round-atomicity claim, for the reported view of it.

3. Both drivers consume the outcome on the failure path, as claimed:
   - `crates/cli/src/remote.rs:91-103`: `sync` returns `(peer.outcome().clone(), result)` regardless of `result` being Err — deliberately, per the doc comment.
   - `crates/cli/src/main.rs:416-424`: `store.export(&format!("sync: {}", remote::describe(&outcome)))` runs *before* `result?`, so the inflated counts become the git commit message and then the error propagates.
   - `crates/server/src/lib.rs:274-287`: the post-session export gate is `!outcome.docs_updated.is_empty() || outcome.log_added > 0 || outcome.threads_added > 0`, driven by the same counters, and they are interpolated into the commit message.

The `dp.baseline` observation is also accurate: it advances inside the closure, so if a `Peer` were reused after a failed round the rolled-back changes would never be re-emitted by `get_changes(&dp.baseline)`. Today both drivers `break` out of the loop and discard the peer on error, so that half is latent rather than live — nothing in the type or the API enforces it.

Severity: I agree with "low". The exported *content* is still whatever the store actually holds (export reads store state, not the counters), so no data is lost or corrupted; the defect is a misleading commit message / stderr line and a spurious export trigger. The latent baseline hazard is what would make it worse, and it is not currently reachable.

</details>

### ⚪ **LOW** · #22 · Every sync session re-exchanges the complete uid list of every log row and thread message ever written, and both sides fully materialize all rows to diff them — O(total history) per session, with no watermark

**`crates/store/src/sync.rs:212`** · _architecture_

`initial_round` sends full `log_uids()`/`thread_uids()`, the responder does the same, and each side loads every row (`thread_rows` parses every ThreadId, Role and DateTime in the store) to filter against a BTreeSet. Threads are the unbounded axis by design — every turn appended forever — while the target device is a phone on bad signal. `Peer::start` also replays every doc's whole Automerge history while holding the server's store mutex. Reported as a designed tradeoff reaching its limit: the doc says "a one-time exchange of log-row uids", settled before threads existed.

- **Spec:** implementation.md, Sync protocol: "a one-time exchange of log-row uids followed by whichever entries the other side lacks".
- **Suggested fix:** Give append-only rows a per-peer high-water mark (persist the last-synced rowid or a session cursor) and exchange only uids above it, falling back to the full list on first contact. Failing that, exchange a digest first so the common no-op session is O(1).

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/store/src/sync.rs:200-223,324-357; crates/store/src/store.rs:712-748


Failure scenario: After a year of daily planning (~15k thread rows), every `mise sync` uploads a ~600 KB uid list and the server deserializes all 15k messages, even when the answer is "already in sync" and zero bytes of real data move.

</details>

---

## Assistant — Anthropic client

**Files:** `crates/assistant/src/client.rs` (hand-rolled SSE + turn assembly), `crates/assistant/src/error.rs`  
**Read first:** implementation.md → *Anthropic client* (M3 + the 2026-08-02 caching / unknown-block policy)  
**Key entry points:** `request_body`, the SSE framer, `absorb`, `stop_reason` mapping  
**Theme:** The streaming machinery is robust; the defects are policy — the clock sits inside the cached block, thinking is on by default under an 8K cap, and a refusal reads as a clean end-turn.

### 🟠 **MEDIUM** · #24 · The wall clock sits inside the single `cache_control`'d system block, so the documented "everything before the clock caches across turns" layering never happens — the tools+system prefix is rewritten on every exchange

**`crates/assistant/src/client.rs:202`** · _spec-drift_

`request_body` emits the system prompt as one text block with `cache_control: ephemeral`, and `context::assemble` appends the clock at minute resolution as that string's last line. Prompt caching is a prefix match and the breakpoint sits at the end of the block containing the clock, so the cache key covers the timestamp; invalidating the system block also invalidates the message-tail breakpoint after it. Within one exchange `now` is read once so the tool loop caches, but across exchanges the minute almost always differs, so BASE plus the rendered State/Steering/Facts pages plus the whole tool schema block are re-written at the 1.25x cache-write premium and never read. The tool-definition breakpoint still works because it closes before `system` renders.

- **Spec:** context.rs module doc: "the clock dead last, so everything before it caches across turns"; implementation.md, Anthropic client: "`cache_control` markers on the system block, the last tool, and the message tail."
- **Suggested fix:** Change `TurnRequest.system` from `String` to a stable-prefix + volatile-tail shape and have `request_body` emit two system text blocks with `cache_control` on the first only; `assemble` returns the clock line as the tail. Assert the real property — the block carrying `cache_control` is byte-identical across two clocks — rather than that the concatenated string shares a prefix. Update context.rs's module doc and implementation.md.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/assistant/src/client.rs:187-189,198-209; crates/assistant/src/context.rs:1-5,108-113; crates/assistant/src/exchange.rs:66-75; crates/assistant/tests/context.rs:37-43


Failure scenario: User messages at 14:07 then 14:09: `assemble` produces a byte-different system string, `cache_read_input_tokens` is 0, and `cache_creation_input_tokens` covers the whole tool schema plus BASE plus three rendered corpus pages again, billed at 1.25x, every exchange.

</details>

### 🟠 **MEDIUM** · #25 · `MAX_TOKENS = 8192` is shared with adaptive thinking, which `claude-opus-5` runs by default because the request never sends a `thinking` field, so a thinking-heavy turn can exhaust the budget before any text and fail the exchange

**`crates/assistant/src/client.rs:20`** · _bug_

`request_body` emits no `thinking` parameter and `DEFAULT_MODEL` is `claude-opus-5`, where omitting it means adaptive thinking at default `high` effort and `max_tokens` caps thinking plus text plus tool JSON together. The assembler and turn.rs already handle thinking blocks, so they really arrive, but nothing was sized for them. Two degradations: `stop_reason: max_tokens` becomes common, and when thinking consumed the budget `absorb` returns a hard `Api` error persisted as a "(no reply — the exchange failed…)" marker; and `thinking.display` defaults to omitted, so `on_delta` fires nothing during a long silent pause.

- **Spec:** implementation.md, Anthropic client — the request-body mapping is the client's contract; no thinking/max_tokens policy is recorded.
- **Suggested fix:** Make the policy explicit: either keep thinking and raise MAX_TOKENS substantially (with `display: summarized` for visible progress) or send `thinking: {"type": "disabled"}`. Record the choice in implementation.md next to the max_tokens note, since the correct value is model-dependent.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Every claim traced and verified. client.rs:198-209 `request_body` emits only model/max_tokens/stream/system/tools/messages — no `thinking`, no `output_config`. client.rs:18 pins `DEFAULT_MODEL = "claude-opus-5"` and line 20 `MAX_TOKENS = 8192`. Per current Anthropic API behavior (claude-api skill, authoritative), Claude Opus 5 is the model where *omitting* `thinking` runs adaptive thinking at default `high` effort — a deliberate change from Opus 4.8/4.7 — and `max_tokens` caps thinking + text + tool JSON together; the documented starting points are ~16000 non-streaming / ~64000 streaming, so 8192 with thinking silently on is undersized. The blocks really do arrive: the assembler has live `Partial::Thinking` accumulation for `thinking_delta`/`signature_delta` (client.rs:383-395, 411-420) and a passing round-trip test (`thinking_blocks_assemble_opaquely_and_round_trip`), so this isn't dead code. The failure chain holds end to end: on `stop == MaxTokens` an unparseable `tool_use` is dropped via `continue` (client.rs:479), leaving `calls` empty; turn.rs:116-126 then takes the non-ToolUse branch and, with `self.reply` empty, returns `AssistantError::Api("the reply was cut off by the length limit before any text arrived")`; exchange.rs:122-135 persists `"(no reply — the exchange failed: {e})"` against an already-persisted user question. The `on_delta` claim also holds — thinking deltas return `Ok(None)`, and `thinking.display` defaults to `"omitted"` anyway, so a thinking-heavy turn is a silent pause. Spec claim verified too: docs/implementation.md:169-188 documents the request-body mapping in detail (version pin, SSE framing, three cache_control markers, timeouts, retries, default model) and records no max_tokens or thinking policy; a grep for thinking/max_tokens/8192 across docs/ hits only the prior review report. One nuance that bounds but does not refute the finding: the hard `Api` error needs thinking to exhaust the budget before any text block — a turn with preceding text takes the softer "(reply cut short — the length limit was hit)" path — but thinking blocks precede text in the response, so the scenario is reachable, and the common `stop_reason: max_tokens` degradation stands regardless. Medium severity is calibrated correctly.

</details>

### ⚪ **LOW** · #26 · The SSE framer's memory bound only covers the line buffer; an event whose `data:` lines never terminate grows `self.data` without limit, bypassing MAX_SSE_BUF

**`crates/assistant/src/client.rs:284`** · _bug_

`push` drains complete lines out of `self.buf` and appends each `data:` payload onto `self.data`, which is only cleared when a blank line completes the event; the `MAX_SSE_BUF` check runs against `self.buf` after the drain loop. A stream of well-formed `data:` lines with no blank line keeps `self.buf` near zero while `self.data` grows unbounded. `self.event` is likewise unchecked. The module comment and implementation.md both assert a bound the code only half provides, and the existing test covers only the no-line-endings case.

- **Spec:** implementation.md, Anthropic client: "streaming SSE with an incremental framer (byte-oriented, line-ending-agnostic, bounded buffer)".
- **Suggested fix:** Account the accumulated event against the same cap (`self.buf.len() + self.data.len() + self.event.len() > MAX_SSE_BUF`), and add a sibling test feeding `"data: x\n"` repeatedly.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced `SseFrames::push` at crates/assistant/src/client.rs:272-297. The drain loop `while let Some(line) = self.next_line()` pops each complete line out of `self.buf` (next_line does `self.buf.drain(..end + skip)` at :331), and for every `data:` line appends the payload onto `self.data` (:286-291). `self.data` is only reset by `std::mem::take` when a blank line completes the event (:278-282). The cap check at :293 is `if self.buf.len() > MAX_SSE_BUF` — it tests only the line buffer, which after the drain loop holds at most the trailing partial line. So a stream of well-formed `data: x\n` lines with no blank terminator keeps `self.buf` at ~0 bytes forever while `self.data` grows by ~2 bytes per line without bound; the guard never fires. No other bound exists in the type: `SseFrames` has exactly the three fields buf/event/data (:264-269) and no other length check appears in the impl. The existing regression test `sse_frames_refuse_a_stream_with_no_line_endings` (:527-535) feeds `b'x'` with no terminators at all, exercising only the `self.buf` path, so the accumulator path is genuinely uncovered. The module comment at :255-257 and the doc claim of a "bounded buffer" therefore overstate what the code enforces.

Two minor corrections to the finding, neither of which changes the verdict: (a) `self.event` is *assigned* (`self.event = v.trim().to_string()`, :285), not appended, so it is effectively bounded by one line's length rather than unbounded — the finding's "likewise unchecked" is overstated for `event` but correct for `data`; (b) the per-line growth is closer to 2 bytes for `data: x` than the quoted ~13, which only lengthens the time-to-OOM, not the existence of the leak. Severity `low` is right: it needs a non-conforming/hostile server on the `--anthropic-base-url` seam, and it is a defense-in-depth bound rather than a bug on the honest path.

</details>

### ⚪ **LOW** · #27 · The `stop_reason` catch-all folds `refusal` into `EndTurn`, so a policy decline surfaces as "the model ended its turn without a reply"

**`crates/assistant/src/client.rs:446`** · _bug_

`message_delta` maps only `tool_use` and `max_tokens`; everything else becomes `EndTurn`. Opus 5 can return HTTP 200 with `stop_reason: "refusal"` and empty content plus a `stop_details` category, so a decline is indistinguishable from a malfunctioning turn and no caller can key a fallback off it. Diagnostic rather than structural, and a true positive is essentially impossible for a household cookbook — the realistic trigger is a false positive on a fetched URL or a pantry photo.

- **Spec:** implementation.md, Anthropic client: unknown SSE events, block types and delta kinds degrade with a stderr warning — a known-but-unmodelled stop reason degrades silently instead.
- **Suggested fix:** Add `StopReason::Refusal` mapped from `"refusal"`, carrying `stop_details.category`, and have `absorb` report a decline; or at minimum capture the raw reason string in the empty-reply error. Keep the catch-all for genuinely unknown reasons.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every claim and each holds.

1. `crates/assistant/src/client.rs:441-448` — the `message_delta` arm matches exactly two reasons and folds everything else into `EndTurn`:
```rust
self.stop = Some(match reason {
    "tool_use" => StopReason::ToolUse,
    "max_tokens" => StopReason::MaxTokens,
    _ => StopReason::EndTurn,
});
```
No `eprintln!` warning here, unlike the sibling arms for unknown content blocks (client.rs:390-393) and unknown delta kinds (client.rs:435-438), which both degrade *with* a stderr warning. So the finding's spec-rule citation is accurate: a known-but-unmodelled stop reason is the one place in this accumulator that degrades silently.

2. `crates/assistant/src/seam.rs:85-90` — `StopReason` has exactly three variants (`EndTurn`, `ToolUse`, `MaxTokens`), so a caller structurally cannot key off a decline. `ModelTurn` carries no raw reason string or `stop_details` either (seam.rs:93-97).

3. `crates/assistant/src/turn.rs:116-131` — with `stop == EndTurn` and no calls, an empty accumulated reply produces exactly `AssistantError::Api("the model ended its turn without a reply")`.

4. The user-visible tail is confirmed too: `crates/assistant/src/exchange.rs:131` and `crates/server/src/chat.rs:187` both write `"(no reply — the exchange failed: {e})"`, so the recorded thread text matches the finding's failure_scenario verbatim.

5. A repo-wide grep for `refusal` / `pause_turn` / `stop_reason` finds no handling of either string anywhere in `crates/` — no missed guard elsewhere, and no test asserting the fold is deliberate. The existing tests only ever feed `tool_use`, `max_tokens`, and `end_turn` (client.rs:615, 648, 712).

Severity `low` is right: this is diagnostic loss, not a correctness or safety defect — the turn still terminates cleanly and the caller still sees an error, just an inaccurate one. Worth noting the same catch-all silently swallows `pause_turn` as well, which would truncate a server-tool turn as if it had ended normally; that is a strictly worse variant of the same one-line gap, though not currently reachable without server tools.

</details>

---

## Assistant — turn, exchange, context

**Files:** `crates/assistant/src/{seam,turn,exchange,context}.rs`  
**Read first:** implementation.md → *The seam concretely*, *Context assembly*, *Tools* (aborted-exchange export + dangling-question marker)  
**Key entry points:** `context::assemble`, the sans-IO `Turn`, `run_exchange`  
**Theme:** Context assembly has no upper bound: thread history and full-corpus renders are re-sent whole, so a long-lived thread eventually fails permanently and each failure makes it longer.

### 🟠 **MEDIUM** · #28 · Thread history is unbounded and re-sent whole on every exchange, so a long-lived thread eventually fails permanently — and each failure appends a marker that makes the thread longer

**`crates/assistant/src/context.rs:115`** · _architecture_

`assemble` builds history from `store.thread_messages(thread)?` — no LIMIT, no windowing, no summarization anywhere in the crate. Every exchange on the permanent planning thread re-sends the entire transcript plus the system prompt plus 19 tool schemas plus up to 32 rounds of tool results. The doc records the deferral of summarization but nothing bounds the failure mode meanwhile: once the request exceeds the context window the API hard-errors, and the failure path appends another message, so the next attempt is strictly longer. There is no trim, archive or new-thread affordance.

- **Spec:** design doc, Graceful decay: "slightly stale suggestions, not a broken database demanding reconciliation."
- **Suggested fix:** Bound the history at the seam before summarization lands: take the last N messages (or N tokens by a character proxy) oldest-first, and note the elision in the system prompt so the model knows to `read_page threads/<id>`. Test as a property: assembled history size is bounded regardless of thread length.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/assistant/src/context.rs:115-122; crates/store/src/store.rs:775-786; crates/assistant/src/exchange.rs:128-133; crates/server/src/chat.rs:184-189


Failure scenario: After a year of planning conversation the assembled request exceeds the context window; every attempt fails and appends "(no reply — the exchange failed: …)", making the next attempt longer. The planning thread becomes permanently unusable with no in-app recovery.

</details>

### ⚪ **LOW** · #29 · `assemble` can hand the model a history whose first message is an assistant turn, which the Messages API rejects with a 400

**`crates/assistant/src/context.rs:118`** · _bug_

`assemble` maps thread rows in `(created, uid)` order with no role check and the drivers push this exchange's user turn on the end. The API requires the first message to be `user` (consecutive same-role turns are fine). Thread rows are a uid-union set merged across replicas with caller-supplied stamps, so nothing guarantees the earliest row is a user turn — an interrupted sync that lands a reply before its question produces exactly that, and it is then permanently wedged. `tests/exchange.rs:368-375` and `:459-466` both seed an assistant-only history and pass, because `Scripted` never validates the request.

- **Spec:** implementation.md, Context assembly: "History is the thread's text turns"; Anthropic Messages API: the first message must use the `user` role.
- **Suggested fix:** In `assemble`, drop leading assistant messages (or return `Protocol` naming the thread). Add a test seeding an assistant-only thread and asserting the assembled history starts with a user turn; change the two existing tests to seed a user/assistant pair.

<details><summary>Verification trail — code pointers</summary>

**Verdict: uncertain** — kept under the lenient bias. The code-level claims are accurate: `assemble` (context.rs:115-123) maps thread rows to ChatMessages with no role check, `Turn::new`/`request()` (turn.rs:61-72) pass the message vector through verbatim, both drivers append the new user turn at the END (exchange.rs:78-80, chat.rs:87-89) so only the last message is guaranteed `user`, and the Messages API does require the first message to use the `user` role. The two cited tests really do seed an assistant-only thread through the public `Store::append_thread_message` API and pass, so the "first row is a user turn" property is not enforced by types or by the store.

However, I could not make the stated trigger reachable. sync.rs:307-319 ingests every thread row a round delivers inside a single `store.transaction`, and the missing-set at sync.rs:334-343 is computed from the peer's complete `thread_uids` list — all rows the peer lacks ship in one round, so an interrupted sync rolls the round back wholesale and cannot persist a reply without its question. Combined with `stamp_after` (exchange.rs:23-31) and the fact that every non-test caller of `append_thread_message` (exchange.rs:77/119/128, chat.rs:86/175/184) writes `Role::User` before any `Role::Assistant`, each replica's earliest row on a thread is a user turn and its reply sorts strictly after its own question; under `(created, uid)` union merge the global minimum is therefore always some replica's user turn. uids embed the replica id (store.rs:694) so cross-replica dedup cannot drop a question while keeping a reply; there is no `DELETE FROM thread_messages`, `migrate_v2_to_v3` (store.rs:1250-1273) drops and recreates rather than synthesizing rows, and there is no markdown import path.

Net: a real latent gap (the invariant is emergent from driver discipline, not structural, and `assemble` would happily emit an invalid request), but the described sync-interruption failure scenario is blocked by per-round transaction atomicity. Not refuted — I cannot rule out other paths (DB-level damage, a future caller of the public append API) — so it survives as uncertain. Severity `low` is right.


Pointers: crates/assistant/src/context.rs:115-123; crates/assistant/src/exchange.rs:75-80; crates/server/src/chat.rs:81-89; crates/store/src/store.rs:775-786; crates/assistant/tests/exchange.rs:368-375,459-466

</details>

---

## Assistant — tools & views

**Files:** `crates/assistant/src/{tools,views}.rs`, `crates/assistant/tests/tools.rs`  
**Read first:** implementation.md → *Tools* (M3), */api/edit* allowlist (M5), first-cook promotion; design doc → *editing & trust model*  
**Key entry points:** `tools::execute`, `shopping_add`, `recipe_edit`, `read_page`, `dish_view`  
**Theme:** The tool layer is mostly the intended single source of truth, but `shopping_add` still trusts a caller id (reopening the collision class), `read_page` renders the whole corpus, and the queue degrade is narrower than the failures it must absorb.

### 🟠 **MEDIUM** · #34 · `read_page` hydrates and renders the entire corpus — every recipe, log month and full thread transcript — to return a single page, while `Store::render_page` exists for exactly this

**`crates/assistant/src/tools.rs:594`** · _architecture_

`read_page` calls `store.corpus()?` then `render::render(&corpus)` and looks up one key. `corpus()` hydrates every location doc, recipe, technique, the whole cook log and every thread (snapshot load plus change replay) and `render()` formats all of them, including unbounded thread transcripts. The identical anti-pattern was fixed one file over: `Store::render_page(&DocId)` now exists and `context::assemble` uses it. `list_pages` has the same shape (it needs paths and recipe metadata, not rendered bodies); only `search` genuinely needs all content. There is also no bound on the returned string, so `read_page threads/planning` returns an unbounded transcript into context.

- **Spec:** implementation.md → Tools (M3): tools are deterministic reads over the store; the #35 remediation established `Store::render_page` as the one-page path.
- **Suggested fix:** Map the requested path to a `DocId` via `DocId::export_path()` and call `store.render_page(&id)`, falling back to the corpus render only for log/thread paths (or add the two missing per-page renderers). For `list_pages`, enumerate with `Store::list(kind)` and hydrate only annotated recipe docs.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/assistant/src/tools.rs:555-633; crates/store/src/store.rs:812-905; crates/store/src/render.rs:84-108; crates/assistant/src/context.rs:90-102


Failure scenario: With 200 recipes and a year-old planning thread, one `read_page {"path": "recipes/mapo-tofu"}` hydrates ~220 Automerge docs, replays their change logs and renders every page plus all transcripts — with the server's single store mutex held, blocking concurrent `/api/queue` and `/api/pages` requests.

</details>

### 🟠 **MEDIUM** · #81 · `shopping_add` honours a caller-supplied `id`, reopening the cross-replica collision class the mint was introduced to close, two lines below the comment saying ids are "minted, never positional"

**`crates/assistant/src/tools.rs:1215`** · _bug_

`shopping_add` uses `a.id` verbatim when present and mints only in the None arm; the schema advertises it ("Item id; generated if omitted") and the rendered shopping page shows ids in its first column, so the model routinely sees them and has an obvious affordance to supply a readable one. Any id derivable from content (`milk`, `eggs`) is by construction reproducible on the other replica, and `ShoppingDoc.items` is an Automerge map. A second edge: a supplied id matching an existing item does a blind `insert`, silently resetting that item's `done` and `tier` — unlike `queue_add`, which deliberately patches and explains why. `/api/edit` forwards `shopping-add` bodies verbatim, so the shape is reachable over HTTP too.

- **Spec:** crates/store/src/store.rs:479-486 — content-derived collection ids "collide across replicas, where the merge resolves both puts to one winner and the other item silently vanishes"; CLAUDE.md → CRDT convergence, the offline shopping-list scenario.
- **Suggested fix:** Drop the `id` field from `shopping_add` (an id is only needed to *address* an existing item, which is `shopping_update`'s job); or, if the escape hatch stays, reject an id not already present and make the existing-id case a patch. Either way the comment at :1216 and the schema hint at :468 must agree with the code.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Every element of the finding checks out against the code.

1. **Caller-supplied id used verbatim.** `tools.rs:1207` declares `id: Option<String>`; `:1215` maps it through `slug`; `:1217-1220` is `match &requested { Some(id) => id.to_string(), None => store.mint_id("s")? }`. The mint is only reached in the `None` arm, exactly as claimed.

2. **The comment contradicts the code.** `tools.rs:1216` reads `// Minted, never positional — see fridge_add.` and sits directly between the caller-id capture and the match that honours it. The referenced `fridge_add` (`:1056-1097`) has *no* `id` field in its `In` struct at all and mints unconditionally at `:1079` — so the cross-reference points at a function that does the opposite of what `shopping_add` does.

3. **The schema advertises it.** `tools.rs:468`: `"id": s("Item id; generated if omitted.")`.

4. **The model sees ids.** `crates/store/src/render.rs:161-173` renders the shopping page as a table with `esc(id)` as the first cell and headers `["id", "item", "tier", "done"]`. So readable/derivable ids are a visible, invited affordance.

5. **The collision class is real and is the one the mint exists to close.** `ShoppingDoc.items` is a `BTreeMap<String, ShoppingItemDoc>` (`crates/store/src/pages.rs:132-135`) — an Automerge map whose keys *are* item identity. `store.rs:479-486` states verbatim that positional/derivable ids "collide across replicas, where the merge resolves both puts to one winner and the other item silently vanishes." Two offline replicas each calling `shopping_add {id:"milk"}` with different text produce two `put`s at the same map key; merge keeps one. The failure_scenario is reachable as written.

6. **The second edge (blind overwrite) is unconditionally true**, not just a merge hazard: `:1223` is a bare `d.items.insert(assigned.clone(), ShoppingItemDoc { text, tier, done: false })` with no `get_mut`/existence check. Any supplied id matching an existing item resets that item's `done` to `false` and replaces its `tier` on a single replica. Contrast `queue_add` (`:677-702`), which explicitly matches `get_mut` first and patches, with a comment explaining why an existing entry must keep its state. `shopping_update` (`:1249-1252`) does guard existence — `shopping_add` is the only shopping path that writes an arbitrary caller key blind.

7. **HTTP reachable.** `crates/server/src/api.rs:226` lists `("shopping-add", "shopping_add")` in `UI_ACTIONS`, and `:255-257` forwards the request body unchanged as the tool input (`Some((_, tool)) => (*tool, body)`) — only `recipe-status` gets a narrowing struct. So the `id` field passes through `/api/edit/shopping-add` untouched.

Severity `medium` is right: the cross-replica loss is contingent on a caller actually supplying a derivable id (invited by the schema, but not forced), while the same-replica `done`/`tier` reset is deterministic whenever an existing id is passed. Neither is a silent-corruption-by-default, but both violate a spec rule stated explicitly in `store.rs:479-486` and in CLAUDE.md's offline shopping-list scenario.

</details>

### 🟠 **MEDIUM** · #85 · The degrade added for a dangling recipe reference covers only NotFound/bad-slug; any doc value that hydrates but will not convert still fails the whole queue view and aborts the exchange

**`crates/assistant/src/views.rs:173`** · _bug_

The fix for "One dangling recipe reference takes down the entire queue view" added `missing()` for `NotFound` and an unparseable slug, but the next line is `let meta = doc.to_core(&s)?;`, which propagates `Corrupt` for `servings == 0`, an out-of-vocabulary `effort`, or `lead.minutes == 0`. `queue_view` also calls `store.active_view()?` → `LocationDocs::to_view`, which errors on a bad `presence`, a malformed date, a non-slug equipment key or tier id, or zero headcount. One such value anywhere in the active location kills `/api/queue` (500), `mise queue` and the `queue_status` tool — and because `Corrupt` maps to `Fail::Store`, it aborts the whole exchange rather than returning an error tool result. The campaign deliberately made recipe status, equipment links and pantry links tolerant precisely because sync applies peer changes verbatim, but left these sibling conversions fallible.

- **Spec:** prior review #40: "The queue is the home screen; the right failure mode is one degraded row"; implementation.md → Schema changes: shape changes ship a permanent tolerant hydrator.
- **Prior:** still-open
- **Suggested fix:** Either give `dish_view` the same degrade for `Err(_)` as for NotFound (a `RecipeUnreadable` verdict naming the field), or push tolerance down by giving `effort`, `presence`, the date fields and the tier/equipment keys the same `repr`-style hydrators status and the slug lists already have. Add a test that a location carrying one bad presence string still renders the queue.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every link of the claim and each holds.

1. `dish_view` only degrades for two cases: an unparseable slug (views.rs:165-167) and `StoreError::NotFound` (views.rs:170); every other `Err` is propagated (views.rs:171), and the very next line `let meta = doc.to_core(&s)?;` (views.rs:173) is fallible.
2. `RecipeDoc::to_core` (pages.rs:420-454) returns `StoreError::Corrupt` for `servings == 0` ("zero servings"), an unparseable `effort` (`self.effort.parse().map_err(corrupt)?`), and `lead.minutes == 0`. `effort` is stored as a plain `String` in the doc ("weekday" | "project"), so an out-of-vocabulary value hydrates cleanly and only blows up here — unlike `status`/`equipment`, which do carry `#[autosurgeon(with = "repr::status")]` / `repr::slug_list` tolerant hydrators (pages.rs:390,397).
3. `queue_view` also calls `store.active_view()?` (views.rs:88) → `Store::active_view` (store.rs:931-936) → `LocationDocs::to_view` (pages.rs:498-540), which is `Corrupt`-fallible on zero headcount, `PantryItemDoc::to_core`'s `presence.parse::<Presence>()` and `bought`/`tier` parsing (pages.rs:198-211), non-slug equipment keys, non-slug tier ids, and `PortionDoc`'s `date` (pages.rs:267-273). One bad value anywhere in the active location's four docs fails the whole view before any per-row degrade can apply.
4. Impact paths are as described: `/api/queue` → `views::queue_view` → `fail(e)`, where `Corrupt` hits the `_ =>` arm and returns 500 (api.rs:29-39, 45-48); `mise queue` → `views::queue_view` (cli/src/main.rs:897); and the tool layer maps only `NotFound|Exists|Invalid|BadDocId` to `Fail::User`, so `Corrupt` becomes `Fail::Store` and `execute` returns `Err(AssistantError::Store(..))`, aborting the exchange rather than returning an error tool result (tools.rs:70-103).
5. The premise that such values can exist is the codebase's own stated model: sync merges peer changes verbatim and "an un-upgraded phone can reintroduce an old shape at any time" (sync.rs:52-60), and pages.rs:24-33 says each shape change ships a permanent tolerant hydrator — which `effort`, `presence`, the date fields and the tier/equipment keys do not have.

Severity medium is right: it needs a peer- or hand-authored out-of-vocabulary value (local write paths validate), but the blast radius is the home screen plus the chat exchange.

</details>

### ⚪ **LOW** · #1 · A dish that needs both a shop trip and a lead time loses its act-now step entirely in the queue view

**`crates/assistant/src/views.rs:175`** · _spec-drift_

`Readiness::verdict` returns `AfterLead(lead)` only when both `missing_equipment` and `shop` are empty (readiness.rs:100-123). `dish_view` builds `VerdictView` from the collapsed verdict and never reads `assessment.lead_time`, so a marinade dish blocked on one staple renders as `shop — Staples: soy-sauce` with no mention that the marinade must start tonight. The information exists in `Readiness` and is discarded one layer above it.

- **Spec:** design.md:62-65 — "the queue surfaces the act-now step instead of silently calling the dish makeable"
- **Suggested fix:** Add `lead: Option<LeadView>` to `DishView` populated from `assessment.lead_time` regardless of verdict, and append it to `dish_line` — or confirm with the user that suppression is intended and say so in design.md.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/core/src/readiness.rs:20-32,100-123; crates/assistant/src/views.rs:49-68,174-195,216-218


Failure scenario: A 12-hour marinade whose only unmet ingredient is a tier-1 staple shows as `shop — Staples: soy-sauce`; neither user nor model is told the marinade still needs starting tonight.

</details>

### ⚪ **LOW** · #36 · `recipe_edit` called with only `slug` changes nothing and still answers "updated recipe X"

**`crates/assistant/src/tools.rs:911`** · _bug_

Every field in the input struct is optional and `clear_lead` defaults to false, so `{"slug": "mapo-tofu"}` runs a `modify` whose closure assigns nothing; `Store::modify` commits only on a real change, so no history entry appears — yet the tool returns an unconditional success string. This is the same defect class as the fixed `shopping_update` case, which was given an explicit guard and a pinning test; `recipe_edit` was not.

- **Spec:** implementation.md → Tools (M3): model-recoverable problems come back as `is_error` results rather than false success.
- **Suggested fix:** Mirror `shopping_update`: if no optional field is present and `clear_lead` is false, return `user("say what changes: title, servings, effort, tags, equipment, ingredients, lead, body, status, or source")`. Alternatively report what actually changed by comparing against the pre-image.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and the defect is real as described.

1. `recipe_edit` (crates/assistant/src/tools.rs:834-912) declares every field but `slug` as `Option<...>`, with `clear_lead: bool` under `#[serde(default)]`. Input `{"slug": "mapo-tofu"}` therefore parses cleanly (nothing is required, and `deny_unknown_fields` isn't triggered by absence).

2. The `store.modify` closure at :873-904 consists entirely of `if let Some(..)` / `if a.clear_lead` arms; with all-`None` input it assigns nothing. `body` is `None`, so the `update_body` call at :908-910 is skipped too.

3. `Store::modify` (crates/store/src/store.rs:343-359) does `let committed = doc.commit_with(...); if committed.is_some() { persist_change(...) }` — an empty change set produces no commit and no persisted history entry, exactly as the finding claims.

4. The function nonetheless ends with an unconditional `Ok(format!("updated recipe {s}"))` at :911, i.e. `is_error: false` and an "updated" claim for a no-op. There is no guard anywhere upstream: dispatch at :537 goes straight to `recipe_edit`, and no other caller pre-validates (server's api.rs:247 always supplies `status`).

5. The contrast the finding draws is accurate: `shopping_update` at :1246-1248 has exactly the guard being asked for (`if !a.remove && a.done.is_none() { return Err(user("say what changes: ...")) }`) plus the pinning test `shopping_update_must_say_what_changes` at crates/assistant/tests/tools.rs:511-518. No analogous test or guard exists for `recipe_edit` (the only slug-only-ish test, tools.rs:102, passes `clear_lead: true`, which is a real change).

Minor caveat that doesn't refute anything: because `deny_unknown_fields` is on, a *mistyped* field name would error rather than silently drop, so the "payload lost" path is narrower than the failure_scenario's phrasing — but a genuinely empty edit call still yields the false success. Severity "low" is appropriate: it requires the model to emit a contentless call, and it corrupts nothing, it just reports a change that didn't happen.

</details>

### ⚪ **LOW** · #37 · `ToolCtx::msg` strips control and whitespace characters but not Unicode bidi/format controls, which survive into the immutable history line the trust model renders

**`crates/assistant/src/tools.rs:52`** · _security_

The filter is `c.is_control() || c.is_whitespace()` — General_Category Cc plus White_Space — so Cf characters (U+200E/200F, U+202A–202E, U+2066–2069) pass through into the change message, which History.svelte renders verbatim (Svelte escapes markup but the browser still honours bidi controls). The action text carries model words (`shopping add {text}`, `log {title}`), and the repo's threat model treats URL-fetched content as hostile with `fetch_url` output flowing into tool arguments. Change history is immutable and replicates to every device, so a spoofed line is permanent. This is residue of the recorded fix, which correctly handles newlines, control characters and length.

- **Spec:** design.md, Editing & trust model: "Every page shows recent changes (what, when, from which conversation)."
- **Suggested fix:** Widen the split predicate to include format characters (Cc/Cf/Zl/Zp, or the explicit bidi ranges) and extend `change_messages_are_one_bounded_line` with a bidi-override case.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced it and the code says what the finding claims. `ToolCtx::msg` (/Users/svein/dev/cookbook/crates/assistant/src/tools.rs:52-65) splits on `c.is_control() || c.is_whitespace()`. Rust's `char::is_control` is General_Category Cc only, and the bidi format characters (U+200E/200F, U+202A-202E, U+2066-2069) are Cf and are not White_Space, so none of them are split points — they survive verbatim into the change message, along with the length truncation applying only at 200 chars. Model-authored text flows straight into that action string: e.g. tools.rs:1222 `ctx.msg(&format!("shopping add {text}"))` where `text` is only `must_trim`ed, and tools.rs:1194 `ctx.msg(&format!("log {}", entry.title))`. I grepped the whole crates/ tree for any normalization/sanitization of these ranges and found none (no hits for bidi/202E anywhere in Rust, Svelte or TS). The rendering side confirms the display path: web/src/lib/components/History.svelte:54 renders `{change.message}` as plain interpolated text — Svelte escapes markup, but bidi overrides are honoured by the browser's UBA and there is no isolation (no `dir`/`unicode-bidi: isolate`/U+2068 wrapping) on `.msg`. The existing regression test crates/assistant/tests/tools.rs:481-494 asserts no `\n`, no `char::is_control`, and ≤200 chars — precisely the Cc-only guard, so a Cf case would pass it today. Impact is display spoofing of an immutable, replicated history line, not corruption of stored provenance, so the "low" severity is appropriately stated.

</details>

---

## Assistant — fetch & recon

**Files:** `crates/assistant/src/{fetch,extract,recon}.rs`  
**Read first:** implementation.md → *fetch_url tool* (M5), *Fetch is a seam*, *Pantry recon* (M6); memory → URL-fetched content is the hostile input  
**Key entry points:** `validate_url`, `redirect_ok`, `extract`, `propose_pantry_diff`  
**Theme:** The v4-mapped bypass is closed; a trailing-dot hostname is the same class of textual-check miss, and the deferred extraction/charset gaps remain accurately open.

### 🟠 **MEDIUM** · #38 · A trailing-dot (fully-qualified) hostname bypasses the local-host refusal: `http://localhost./` and `http://printer.local./` pass `validate_url` and resolve to loopback/mDNS

**`crates/assistant/src/fetch.rs:85`** · _security_

The `Host::Domain` arm compares the lowercased host against `localhost`, `.localhost`, `.internal` and a `.local` suffix. DNS names may carry a trailing root dot and the `url` crate preserves it, so none of the four comparisons match and the URL is accepted; resolvers strip the root label, so the name still resolves. Verified against the workspace's url 2.5.8 and the platform resolver: `http://localhost./x` → Ok with host `Domain("localhost.")`, and `("localhost.", 80).to_socket_addrs()` → `[::1]:80, 127.0.0.1:80`. `redirect_ok` shares the predicate, so a public page can 302 into it. Distinct from the deferred resolve-and-pin item: this is the textual control failing on its own terms, with a one-line fix.

- **Spec:** implementation.md, *`Fetch` is a seam like `Model`*: "The host check is **textual, not resolved**" — the textual check is the guarantee that is claimed.
- **Suggested fix:** Strip a single trailing dot before matching (`let d = d.to_ascii_lowercase(); let d = d.strip_suffix('.').unwrap_or(&d);`) and add `http://localhost./x`, `http://foo.localhost./x`, `http://printer.local./x` to `url_policy_rejects_the_obvious` plus a trailing-dot hop to the redirect test.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced and empirically verified. `validate_url`'s `Host::Domain` arm (crates/assistant/src/fetch.rs:83-93) lowercases the host and does exact/suffix matches against `localhost`, `.localhost`, `.local`, `.internal` — none of which tolerate the DNS root dot. I compiled a throwaway integration test against the workspace's own url 2.5.8 and the crate under test (then deleted it): `url::Url::parse("http://localhost./x")` yields host `Domain("localhost.")`, `mise_assistant::fetch::validate_url("http://localhost./x")` returns `Ok(())`, `validate_url("http://printer.local./x")` returns `Ok(())`, and `("localhost.", 80).to_socket_addrs()` resolves to `[[::1]:80, 127.0.0.1:80]`. So the textual refusal is bypassed while the name still resolves to loopback. `redirect_ok` (fetch.rs:136-141) delegates to the same `validate_url`, so a public page can 302 into `http://localhost./...` and the reqwest custom redirect policy (fetch.rs:161-166) will follow it. The existing test `url_policy_rejects_the_obvious` (fetch.rs:226-248) covers only the dotless spellings, so nothing catches this. Note the finding's own scope caveat is fair: the IP-literal arms are unaffected, and this is the textual control failing on its own stated terms, not the deferred resolve-and-pin work. Severity medium is right for a personal-server threat model where the doc explicitly disclaims being a bulletproof SSRF boundary — but the fix is a one-liner strip of a single trailing dot before matching.

</details>

### ⚪ **LOW** · #39 · `Proposal`/`ProposalLine` are the only model-supplied tool inputs in the crate without `#[serde(deny_unknown_fields)]`, so `propose_pantry_diff` silently discards fields it does not understand

**`crates/assistant/src/recon.rs:105`** · _spec-drift_

All 17 tool input structs in tools.rs carry `deny_unknown_fields` (the Phase 5 remediation "Tool inputs reject what they don't understand"). `propose_pantry_diff` is parsed from the same untrusted model JSON via `serde_json::from_value` at recon.rs:130-131 but neither struct opts in, so extra keys are dropped without a word to the model — a green tool result for a line that carries less information than the model believes.

- **Spec:** docs/remediation.md, Phase 5: "Tool inputs reject what they don't understand — #37, #39"; implementation.md M6: "Each proposal line is exactly one `pantry-set` tap."
- **Suggested fix:** Add `#[serde(deny_unknown_fields)]` to both structs and extend `bad_proposals_come_back_as_model_facing_errors` with an extra-key case. (Both types are also Serialized into the SSE event; deny_unknown_fields affects only deserialization, so the wire format is unchanged.)

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/assistant/src/recon.rs:104-131; crates/assistant/src/tools.rs:589 (+16 more)


Failure scenario: The model sends a line with `tier` and `quantity`; the tool replies "Proposal shown to the user as 1 tappable lines" and the model summarizes the tier in its reply, but the SSE event and the resulting tap carry only item/presence/reason.

</details>

### ⚪ **LOW** · #40 · `recipeIngredient` is rendered only when it is a JSON array, so a single-string ingredient list is dropped and the recipe still wins over Readability

**`crates/assistant/src/extract.rs:145`** · _bug_

`render_recipe` matches `if let Some(Value::Array(lines))`. schema.org properties are single-or-repeated by definition and the rest of the module handles both spellings (`str_of` recurses through arrays and accepts a bare string; `instruction_lines` handles String, Array and Object). Because the title still renders, the output is non-empty, so the `md.is_empty()` guard never fires and `readable_article` is never consulted — the ingredients are lost rather than recovered by the fallback.

- **Spec:** implementation.md, *`fetch_url` tool*: "schema.org `Recipe` JSON-LD when present … exact ingredients/steps"
- **Suggested fix:** Normalize to a list before rendering (a small `as_list(&Value) -> Vec<String>` reusing `str_of` covers both spellings) and add a fixture with a string-valued `recipeIngredient`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Verified against the real source. `render_recipe` (extract.rs:145) narrows with `if let Some(Value::Array(lines)) = recipe.get("recipeIngredient")` — a `Value::String` (or `Value::Object` with `@type: ItemList`) simply fails the pattern and no Ingredients section is written; nothing downstream recovers it. This is inconsistent with the rest of the module, which deliberately handles the single-or-repeated schema.org shape: `str_of` (74-82) accepts a bare String, a Number, and recurses into Arrays, and `instruction_lines` (100-118) handles String/Array/Object. So `recipeInstructions:"Simmer and serve."` renders fine while `recipeIngredient:"200 g tonnarelli"` is silently dropped. The output is still non-empty (the `# {title}` header at line 125 is unconditional, `unwrap_or_else(|| "Recipe")`), so `extract`'s `md.is_empty()` check at line 24 never fires and the Readability path is never reached — note that guard would return `Err("the page had no readable content")` anyway, not fall back to `readable_article`, since the fallback at line 21 only runs when `json_ld_recipe` returns `None`. Either way the ingredients are lost. Test fixture JSON_LD_PAGE (line 197) only exercises the array spelling, so no coverage. Severity "low" is appropriate: the single-string spelling is legal schema.org but uncommon in practice, and the failure is a silently incomplete recipe rather than a crash or wrong data.

</details>

### ⚪ **LOW** · #42 · ISO-8601 durations containing days or seconds still render as "0 min" — deferred by decision after M7

**`crates/assistant/src/extract.rs:90`** · _bug_

`duration` reads only `span.get_hours()` and `span.get_minutes()` and jiff spans are unbalanced by construction, so `P2D` → "0 min", `P1DT2H` → "2 h" (day dropped), `PT45S` → "0 min". Unchanged since the prior review; recorded in implementation.md → Known, scheduled after M7.

- **Spec:** implementation.md, *`fetch_url` tool*: schema.org Recipe JSON-LD rendered faithfully.
- **Prior:** deferred-by-decision
- **Suggested fix:** Deferred. When revisited: convert with `span.total(Unit::Minute)` and format from that, with `P2D`/`P1DT2H`/`PT45S` fixtures.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and it says exactly what the finding claims. `crates/assistant/src/extract.rs:85-96` parses the ISO-8601 string into a `jiff::Span`, then at line 90 reads only `span.get_hours()` and `span.get_minutes()`, formatting from those two fields alone. jiff Spans are unbalanced by construction — parsing preserves the units as written and does not roll days into hours or seconds into minutes — so `P2D` yields days=2/hours=0/minutes=0 and falls into the `(0, m)` arm rendering "0 min"; `P1DT2H` yields days=1/hours=2 and renders "2 h" with the day silently dropped; `PT45S` yields seconds=45 and renders "0 min". No guard, no balancing call (`span.total(Unit::…)` appears nowhere in the repo — the only `get_hours` hit in the codebase is this line), and the fall-through at line 87-88 only catches strings that fail to parse, which `P2D` does not. `render_recipe` at lines 133-137 feeds `prepTime`/`cookTime`/`totalTime` straight through `duration` into the `- Total: …` facts list, so the fabricated figure reaches the model verbatim. The failure_scenario is reproducible as written. The finding is also self-consistent about its status: it is recorded as deferred in `docs/implementation.md:471-479` ("three extraction-quality gaps (ISO-8601 durations losing whole days …)"), and the prior review `docs/reviews/2026-07-31-codebase-review.md:1581-1598` documents the same three fixtures and the same suggested fix. Severity "low" is fair — it is a rendering-fidelity gap on fetched external pages, touches no stored data, and misreports only in the uncommon multi-day/sub-minute cases.

</details>

### ⚪ **LOW** · #43 · An empty JSON-LD Recipe husk still beats the real article — deferred by decision after M7

**`crates/assistant/src/extract.rs:19`** · _bug_

`extract` takes the JSON-LD branch whenever `json_ld_recipe` returns Some, and `render_recipe` never checks for substance: a `{"@type":"Recipe","name":"X"}` stub renders a non-empty heading, so the emptiness guard does not fire and `readable_article` is never consulted. Unchanged since the prior review.

- **Spec:** implementation.md, *`fetch_url` tool*: JSON-LD when present, else Readability extraction.
- **Prior:** deferred-by-decision
- **Suggested fix:** Deferred. When revisited: treat a Recipe with neither `recipeIngredient` nor `recipeInstructions` as no match, keep scanning remaining `ld+json` blocks, and fall through to `readable_article`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and it behaves exactly as described. `extract` (extract.rs:19-22) matches on `json_ld_recipe(html)` and takes the JSON-LD branch on any `Some`, never consulting `readable_article`. `json_ld_recipe` (42-53) returns `Some(render_recipe(recipe))` on the *first* block where `find_recipe` succeeds, and `find_recipe` (56-72) tests only `@type == "Recipe"` — no check for `recipeIngredient` or `recipeInstructions`. `render_recipe` (120-163) unconditionally writes `# {title}\n\n`, falling back to the literal "Recipe" when `name` is absent (124-125); the ingredient block is gated on `Some(Value::Array(..))` (145) and the method block on `!steps.is_empty()` (156), so a husk like `{"@type":"Recipe","name":"X"}` renders exactly `# X\n\n`. That trims to `# X`, which is non-empty, so the `md.is_empty()` guard at line 24 cannot fire, and line 35 appends the Source line. Result is a bare heading plus Source, with the article body in the same HTML never reaching the model — the stated failure_scenario. Also note the early-return means remaining `ld+json` blocks are not scanned, matching the suggested fix. docs/implementation.md:471-479 independently records this same item ("an empty JSON-LD husk beating Readability") under "Known, scheduled after M7", which corroborates the finding and its deferred status rather than refuting it. Severity "low" is appropriate: quality degradation on an uncommon page shape, no stored-data impact.

</details>

### ⚪ **LOW** · #44 · The response charset is ignored — `from_utf8_lossy` over the accumulated bytes; deferred by decision after M7

**`crates/assistant/src/fetch.rs:195`** · _bug_

`HttpFetch::fetch` accumulates raw bytes for the 2 MB cap then calls `String::from_utf8_lossy`, discarding both the Content-Type charset and any `<meta charset>`. windows-1252/ISO-8859-1 pages turn every non-ASCII byte into U+FFFD, corrupting the JSON-LD parse and any drafted page. Unchanged since the prior review.

- **Prior:** deferred-by-decision
- **Suggested fix:** Deferred. When revisited: capture the charset from Content-Type before streaming and decode with `encoding_rs` (already in reqwest's tree), falling back to a `<meta charset>` sniff on the first chunk.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced it. `HttpFetch::fetch` (crates/assistant/src/fetch.rs:178-196) sends the request, checks status, then streams chunks into a `Vec<u8>` until `MAX_HTML` and returns `String::from_utf8_lossy(&bytes).into_owned()` at line 195. Nothing anywhere reads `resp.headers()` for Content-Type/charset, and a repo-wide grep for `charset` / `encoding_rs` across `crates/` returns zero hits (only `from_utf8_lossy` uses in this file, client.rs SSE line-splitting, and test harnesses). The `Cargo.toml` tree has no direct `encoding_rs` dependency either, so no decoding happens downstream — `extract::extract` receives the already-lossy `String`. So a windows-1252 / ISO-8859-1 page does turn every non-ASCII byte (e.g. 0xE6 for æ) into U+FFFD before any parsing, exactly as described.

Two calibration notes, neither of which refutes the finding: (1) the failure_scenario's "the parse fails" for the `ld+json` block is slightly overstated — U+FFFD inside a JSON *string value* is still valid JSON, so serde would typically succeed with garbled text rather than fail; corruption of drafted page content is the reliable consequence, hard parse failure only when replacement bytes land in structural positions. (2) The prior_status is accurate: docs/implementation.md:471-478 ("Known, scheduled after M7") explicitly lists "an ignored response charset" among three extraction-quality gaps deliberately not fixed, noting none touches stored data. Low severity is right — degraded extraction quality on non-UTF-8 pages, no persistence or safety impact.

</details>

### ⚪ **LOW** · #45 · Hostnames are validated textually with no resolve-and-pin, so a public name resolving into a private range is still fetched — deferred by decision after M7

**`crates/assistant/src/fetch.rs:83`** · _security_

`validate_url` checks `Host::Domain` against four literal suffixes and never resolves; reqwest then resolves and connects, so `http://127.0.0.1.nip.io:8080/` reads a loopback service into the model's context and a rebinding answer wins the TOCTOU window. The docs and the module comment now agree on the weaker guarantee and name the systemd sandbox as the second line.

- **Spec:** implementation.md, *`Fetch` is a seam*: "Resolve-and-pin … is scheduled after M7 … Until then the systemd sandbox is the second line."
- **Prior:** deferred-by-decision
- **Suggested fix:** Deferred (#45). When revisited: install a custom resolver/connector validating every resolved `IpAddr` against `reject_private_v4`/the v6 predicate and connect to the pinned address. The trailing-dot bypass is independent and should not wait for this.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code says exactly what the finding claims. `validate_url` (crates/assistant/src/fetch.rs:76-114) parses the URL and, for the `Host::Domain` arm (lines 83-93), lowercases the host and rejects only `localhost`, `.localhost`, `.local`, `.internal`, then returns `Ok(())`. No name resolution happens anywhere in the module — a grep for `resolve`/`resolver`/`dns`/`pin` across `crates/assistant/src/` finds only unrelated `resolve_location`/`resolve_tier` in tools.rs. The IP-literal arms (94-113) and `reject_private_v4` (116-130) apply only when the host parses as an IP literal, so a name like `127.0.0.1.nip.io` or `169.254.169.254.nip.io` parses as `Host::Domain`, clears all four suffix checks, and reaches `HttpFetch::fetch` (177-197), where reqwest does the resolution and connects. The redirect policy (161-166) calls the same `redirect_ok` → `validate_url`, so it inherits the identical textual-only weakness; nothing in the client builder installs a custom resolver or connector, so there is no post-resolution address check and no pinning — validation strictly precedes connect, leaving the rebinding TOCTOU window the finding describes. The `Fetch` seam trait (13-18) takes a `&str` URL, so no layer below sees the resolved address either. docs/implementation.md:309-320 states the same guarantee explicitly ("The host check is **textual, not resolved**… Resolve-and-pin … is scheduled after M7 … Until then the systemd sandbox is the second line"), which matches the `prior_status: deferred-by-decision`. The failure scenario is reachable as written. Severity `low` is right for this codebase: it is a documented, accepted deferral in a single-user personal server with a sandbox as the second line, and the fetch is model/user-driven rather than attacker-supplied by default — but the defect itself is real, not refuted. Side note supporting the finding's own `suggested_fix`: the trailing-dot form (`http://localhost./x`) is also independent of resolution — `d == "localhost"` is false and none of the three `ends_with` suffixes match a trailing `.`, so it passes the domain arm.

</details>

### ⚪ **LOW** · #84 · `fetch_url` enforces the "only URLs the user gave you" rule in prose alone, and the remediation campaign's completion accounting does not list it among the deliberate deferrals

**`crates/assistant/src/fetch.rs:30`** · _security_

`execute_fetch` validates scheme and address literal and nothing else: no check that the URL appeared in a user-authored turn, no per-exchange fetch cap, no logging of fetched URLs. Fetched page text re-enters the same conversation as a tool result alongside steering and facts, so injected instructions can steer a follow-up fetch with an attacker-chosen query string as the exfiltration channel. Reported for bookkeeping: implementation.md:315-320 schedules this and resolve-and-pin after M7, but remediation.md:13-15 names exactly five out-of-scope findings and claims all 89 in-scope findings fixed — the two fetch findings are in neither list and in no phase checklist, yet the code is unchanged. Everything else verified in this file is genuinely fixed and well tested (v4-mapped respellings route through `reject_private_v4`, CGNAT/0.0.0.0-8/broadcast/documentation covered, `redirect_ok` testable).

- **Spec:** implementation.md:315-320 — enforcing that a fetched URL came from a user turn is scheduled after M7; remediation.md:13-15 — "Five findings are deliberately out of scope".
- **Prior:** deferred-by-decision
- **Suggested fix:** No code change if the deferral stands, but reconcile the docs: add these two findings to remediation.md's deferral list and implementation.md's "Known, scheduled after M7". When the work lands, the cheap half is a per-exchange fetch cap plus an `info!` naming each fetched URL.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Code claim verified: execute_fetch (fetch.rs:21-43) runs only validate_url — scheme + host/IP-literal predicate (fetch.rs:76-130) — before calling fetch.fetch(url). No provenance check tying the URL to a user turn (the function has no access to the thread at all, only ToolCall), no per-exchange fetch counter, and no logging (the module imports no tracing/log). Extracted page text returns as an ordinary ToolOutcome into the same context that carries steering and household facts, so the injected-follow-up-fetch exfiltration channel described is real. Doc/bookkeeping claim also holds up on the part I could check positively: implementation.md:316-319 schedules both resolve-and-pin and "enforcing in code that a fetched URL came from a user turn" after M7; remediation.md:13-15 names exactly five out-of-scope findings (#2, #45, #49, #51, #52) and points at implementation.md's "Known, scheduled after M7" section, which I read at implementation.md:471-479 — it enumerates exactly five items (rotation tool, slugify non-ASCII, ISO-8601 durations, empty JSON-LD husk, ignored charset) and none of them is a fetch-security item. So the fetch deferrals are provably not among the recorded five, while the campaign header claims all 89 in-scope findings fixed. Soft spot, noted for honesty but not disqualifying: remediation.md references every finding id 0-94 except #11, and Phase 1 (remediation.md:46) closes a "honest fetch guarantee" doc half as #48, plausibly the textual-hostname fetch finding resolved doc-only; I could not determine whether the "fetch_url will fetch any URL the model emits" finding also has an id buried in a phase line, because the id-to-heading mapping in the review is not document order (known anchors give drifting offsets of 0, 1, 2 and 5). That ambiguity affects only the ledger detail, not the code state. Everything else the finding credits as fixed I confirmed: v4-mapped/v4-compatible respellings route through reject_private_v4 (fetch.rs:100-102), CGNAT/0.0.0.0-8/broadcast/documentation are covered (fetch.rs:116-130), and redirect_ok (fetch.rs:136-141) is testable and tested.

</details>

---

## Server

**Files:** `crates/server/src/{lib,api,chat,main}.rs`, `crates/server/tests/*.rs`  
**Read first:** implementation.md → *Auth* (M2), *Server defaults*, *The JSON API* (M4), */api/edit* (M5), *Surfaces*  
**Key entry points:** the auth tower layer, `handle_sync`, `/api/edit/{action}`, the SIGTERM drain  
**Theme:** Auth-as-a-layer landed cleanly; the drain that motivated the SIGTERM work does not actually cover the WebSocket sync sessions it was built for.

### 🟠 **MEDIUM** · #46 · The graceful SIGTERM drain does not cover WebSocket sync sessions, so a stop cuts an in-flight sync and skips its post-session export

**`crates/server/src/lib.rs:227`** · _bug_

`ws.on_upgrade(...)` runs the callback in a detached `tokio::spawn`, so the HTTP connection task `axum::serve` tracks for graceful shutdown completes at the upgrade handoff and drops its `close_rx` clone. On SIGTERM main returns immediately, the runtime is dropped, and a `handle_sync` parked on `socket.recv().await` never reaches `peer.outcome()` or `store.export(&message)`. Applied rounds are committed in SQLite, so no corpus data is lost, but the markdown export and its git history are left behind the store until the next server-side mutation — precisely the class the SIGTERM work exists for. `/chat` is genuinely drained (its SSE body keeps the tracked connection alive); only `/sync` looks drained and is not.

- **Spec:** implementation.md, Server defaults: "the case worth draining for is a stop landing inside the export's rewrite-then-commit sequence."
- **Suggested fix:** Give the WS session an explicit shutdown path: a `CancellationToken` in `AppState`, `select!`ed against `socket.recv()` in `handle_sync` so the loop breaks and the post-session export still runs, plus a `TaskTracker`/`JoinSet` the shutdown future waits on with a deadline below `TimeoutStopSec=30`. Regression test: open a real `/sync` socket, complete one round, SIGTERM, assert the export reflects the pushed doc.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every link in the chain and it holds.

1. `crates/server/src/lib.rs:227` — `ws.on_upgrade(move |socket| handle_sync(state, socket))`. In axum 0.8.9 (`src/extract/ws.rs:359`) `on_upgrade` unconditionally does a bare `tokio::spawn(async move { ... callback(socket).await; })`. The session future is detached; nothing in `AppState` or the handler retains a handle to it.

2. `axum-0.8.9/src/serve/mod.rs:389-415` — the per-connection task holds the `close_rx` clone and drops it as soon as `serve_connection_with_upgrades` resolves. Hyper's upgradeable connection future resolves at the upgrade handoff (the IO is passed to the pending `OnUpgrade`), so the tracked connection is finished the moment the 101 is written. `close_tx.closed().await` at `serve/mod.rs:303` — the entire graceful drain — therefore never observes the WS session. `conn.graceful_shutdown()` on an already-upgraded connection is also a no-op.

3. `crates/server/src/main.rs:133-136` — the only shutdown mechanism is `with_graceful_shutdown(shutdown)`; when it returns, `#[tokio::main]` returns and the runtime is dropped, aborting all detached tasks. A `handle_sync` parked at `socket.recv().await` (lib.rs:244) never falls out of the loop, so lines 274-287 — `peer.outcome()` and `store.export(&message)` — never run.

4. No mitigating infrastructure exists: `grep` for `CancellationToken`/`TaskTracker`/`JoinSet`/`tokio_util` across `crates/server/src` and `crates/server/Cargo.toml` returns nothing. There is no shutdown channel in `AppState`, and `handle_sync` has no `select!` against anything.

5. `crates/server/tests/shutdown.rs:83-123` only asserts the process exits 0 on SIGTERM with no client connected — it cannot catch this. The contrast with `/chat` is right too: `chat_endpoint` (lib.rs:168+) returns an SSE body, so its connection task stays alive and is genuinely tracked by the drain.

Impact is as described and bounded: rounds already applied are durable in SQLite, so the loss is the markdown export plus its git commit lagging the store until the next server-side mutation triggers an export. That is exactly the rewrite-then-commit window the SIGTERM work in main.rs:141-156 was written for, so medium stands.

</details>

### ⚪ **LOW** · #21 · The server never surfaces `peer_is_newer()`, so an un-upgraded server silently accepts a newer peer's changes and writes a degraded export with no warning

**`crates/server/src/lib.rs:289`** · _spec-drift_

The spec's justification for putting `schema` on the wire is that a shape crossing a build boundary must be legible — "a warning plus a `SyncOutcome` field is enough". `peer_schema`/`peer_is_newer()` exist and the CLI warns (remote.rs:149-158); `handle_sync`'s completion log reports counts only. The server is the side owning the git-committed export, so it is the one place that would tell an operator to upgrade.

- **Spec:** implementation.md, Schema changes: "Sync is a shape boundary … a peer's shape must be legible before its changes are applied."
- **Suggested fix:** In `handle_sync`, `warn!` when `outcome.peer_is_newer()`, naming both versions, and mention it in the export commit message so the degraded export carries its own explanation.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/server/src/lib.rs:274-295; crates/store/src/sync.rs:171-178; crates/cli/src/remote.rs:149-158


Failure scenario: A schema-2 CLI syncs a new recipe field to a schema-1 server; the server applies it, exports through hydrators that don't know the field, and git-commits a silently lossy backup with nothing in the logs saying why.

</details>

### ⚪ **LOW** · #30 · The server's exchange reads wall time directly instead of taking the clock as an input, so its stamping path cannot be driven by a scripted clock the way `run_exchange`'s can

**`crates/server/src/chat.rs:65`** · _quality_

`run_exchange` takes a `clock` parameter precisely so its time behaviour is testable, and two clock-anomaly properties depend on it. The server's mirror calls `Zoned::now()` inline twice (`:65` for ToolCtx.now and the question stamp, `:164` for the reply stamp), so `crates/server/tests/chat.rs` cannot reproduce a stalled or stepped-back clock. `stamp_after` is shared, so the arithmetic is covered but the server's wiring of it is asserted nowhere — and the server is the copy most exposed to concurrent writers, since the store mutex is released across model calls.

- **Spec:** CLAUDE.md, Time is an input: "no logic reads wall time."
- **Suggested fix:** Give `chat::exchange` the same `clock` parameter shape as `run_exchange` (supplied by `drive`, defaulting to `Zoned::now`), and add a server-side test driving a frozen/backwards clock through the SSE path.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/server/src/chat.rs:65,164; crates/assistant/src/exchange.rs:23-31,55-64; crates/assistant/tests/exchange.rs:151-173,363-396


Failure scenario: A clock anomaly that scrambles the server-side transcript ordering would not be caught by any test, because the server path cannot be driven by a scripted clock.

</details>

### ⚪ **LOW** · #41 · A fresh `reqwest::Client` is built for every chat request, even for the majority of exchanges that never call `fetch_url`

**`crates/server/src/chat.rs:94`** · _quality_

`HttpFetch::new()` runs once per `/chat` request, unconditionally, before the tool loop; it constructs a rustls `ClientConfig`, a fresh root certificate store and a new connection pool each time, against reqwest's own advice to create one client and reuse it. The CLI and evals do the same but are one-shot processes; the long-lived server is the case the advice targets.

- **Suggested fix:** Build one `HttpFetch` at startup and hold it in `AppState` (`reqwest::Client` is already cheap to clone/internally shared); adjust the `Fetch` seam to `&self` or clone per exchange. Leave the CLI and evals as they are.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/server/src/chat.rs:94; crates/assistant/src/fetch.rs:158-175; crates/server/src/lib.rs:49-60


Failure scenario: Every chat turn pays construction of a rustls config and root store, and two `fetch_url` calls in consecutive turns to the same host perform two full TLS handshakes instead of reusing a pooled connection.

</details>

### ⚪ **LOW** · #47 · An explicit `--anthropic-key-file` that is missing or unreadable is silently ignored and the server boots sync-only, while the token reader in the same file treats the same situation as fatal

**`crates/server/src/main.rs:52`** · _bug_

`read_anthropic_key` guards with `Some(p) if p.exists()` and discards read errors with `.ok()?`; on failure control falls to `$ANTHROPIC_API_KEY` and then to None, reported as a single info line. So a wrong path (typo, renamed agenix secret, unmaterialized LoadCredential) or a 0400 root-owned secret is indistinguishable from "no key configured", and the two failure modes behave differently for no stated reason. `read_token` directly below uses `with_context` and hard-fails. Symptom: every `/chat` answers 503 with one info line in the journal and no non-zero exit for the supervisor.

- **Spec:** implementation.md, Surfaces: "The key arrives like the bearer token: `--anthropic-key-file`, then `$CREDENTIALS_DIRECTORY/anthropic`, then `$ANTHROPIC_API_KEY`" — the token path errors on an unreadable explicit file.
- **Suggested fix:** Make an explicitly passed `--anthropic-key-file` fatal on any read error (return `Result<Option<String>>`, propagating with context); only the implicit `$CREDENTIALS_DIRECTORY/anthropic` probe should fall through on absence. Symmetrically, let `read_token` fall back to `$MISE_TOKEN` when the credential is absent, as the doc and clap help promise.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code; it says what the finding claims.

`read_anthropic_key` (crates/server/src/main.rs:47-57) returns `Option<String>` with no error channel at all:
- `Some(p) if p.exists() => std::fs::read_to_string(&p).ok()?` — a missing explicit `--anthropic-key-file` fails the `p.exists()` guard and falls through the `_` arm to `$ANTHROPIC_API_KEY`, then to `None` if unset. An existing-but-unreadable file (EACCES on a 0400 root-owned secret) hits `.ok()?` and yields `None` directly. Either way an explicitly-passed path that cannot be used is indistinguishable from "no key configured".
- main.rs:115-125 handles `None` with a single `info!("no Anthropic key; running sync-only")` and continues to bind and serve; there is no non-zero exit, so a supervisor sees a healthy unit.
- lib.rs:169-172 then answers every `POST /chat` with 503 `"no model configured on this server"`, exactly the described symptom.

The asymmetry is real: `read_token` directly below (main.rs:59-77) uses `with_context(...)?` on the same kind of read and hard-fails, and `read_token(&args)?` at main.rs:99 aborts startup. (Also as the finding's fix note says, `read_token`'s `Some(p)` arm never falls back to `$MISE_TOKEN`, which the clap help at main.rs:19-20 and docs/implementation.md:192-195 both promise — secondary, but it corroborates that the two readers implement the same documented three-step chain differently.)

docs/implementation.md:192-195 does state the key "arrives like the bearer token", so the spec_rule quote is accurate; the doc does not say a broken explicit path should be silently downgraded.

One small imprecision in the detail text, not enough to refute: on a *read error* control does not fall to `$ANTHROPIC_API_KEY` — `.ok()?` returns `None` from the function immediately. The fall-through to the env var only happens for the non-existent-path case. Both still end in the same silent sync-only boot.

Severity "low" is fair — it degrades to a clearly-logged-but-easily-missed state rather than corrupting data or leaking the key.

</details>

### ⚪ **LOW** · #48 · Four of the nine allowlisted `/api/edit` actions have neither a client nor a test, so their action→tool mapping is unverified

**`crates/server/src/api.rs:219`** · _test-gap_

`UI_ACTIONS` maps nine action names to tool names by string; only `equipment-set`, `pantry-set` and the special-cased `recipe-status` are exercised anywhere, and `fridge-add`, `fridge-remove`, `shopping-add`, `shopping-update` have no test and no web caller. A typo in either half is not a compile error and not a test failure — `tools::run` falls through to `no such tool: {other}`, which becomes a 400, so the allowlist looks fine while a whole action is dead. The allowlist is this surface's security boundary. Related inaccuracy in the same comment block: api.rs:216-217 calls these edits idempotent and replayable, but `shopping_add`/`fridge_add` mint a fresh id when none is supplied, so a replayed tap duplicates rather than converges.

- **Spec:** implementation.md: "An allowlist (pantry, equipment, fridge, shopping, recipe-status) maps each action to the matching tool under `ui:` provenance."
- **Suggested fix:** Add a table-driven test posting a minimal well-formed body to every `UI_ACTIONS` entry (plus recipe-status) and asserting none answers "no such tool" — a mapping-integrity test, not per-tool behaviour. Either wire up or delete the fridge/shopping actions, and correct the "idempotent" claim.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/server/src/api.rs:216-228,254-259; crates/assistant/src/tools.rs:549,1201-1231; crates/server/tests/api.rs:107-183


Failure scenario: `("shopping-update", "shopping_updat")` compiles, passes cargo test and the auth route table; the first tap from a future shopping screen gets `400 {"error":"no such tool: shopping_updat"}` in the kitchen with nothing in CI having noticed.

</details>

### ⚪ **LOW** · #49 · With `--static-dir`, an unknown `/api/...` path answers 200 text/html (the SPA shell) instead of 404

**`crates/server/src/lib.rs:140`** · _quality_

`router.fallback_service(serve)` hangs the SPA fallback on the outer router, so any unmatched path — including everything under `/api/` — returns index.html with status 200 and no authentication. Not an auth bypass (no handler runs and the shell is already public at `/`), but the shared client decides on status then parses JSON, so a mistyped or renamed API route surfaces as a JSON parse error against HTML rather than a 404. The API is also inconsistent with itself: `/api/edit/nope` returns a proper 404 JSON body while `/api/nope` returns the web app.

- **Spec:** implementation.md, The web app: "served whole by `mise-server --static-dir` with index.html fallback" — specified for app routes, not the bearer-authed `/api` namespace.
- **Suggested fix:** Register an `/api/{*rest}` (and `/chat`, `/sync`) 404 JSON fallback on the authed sub-router, or scope the SPA fallback so only non-API paths fall through; pin it with a test asserting `GET /api/nope` is 404 JSON on a `spawn_with_static` server.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/server/src/lib.rs:82-141; crates/server/src/api.rs:257; crates/server/tests/auth.rs:112-130


Failure scenario: `GET /api/pagez` returns 200 with `<!doctype html>…`; `request()` sees success and throws inside `res.json()`, so the UI reports a parse failure instead of "not found".

</details>

---

## CLI & remote

**Files:** `crates/cli/src/{main,remote}.rs`, `crates/cli/tests/{cli,remote}.rs`  
**Read first:** implementation.md → *Library choices* (CLI is an edge), *Server defaults* (join flow), *Surfaces*  
**Key entry points:** `run_pantry`/`run_fridge`/`run_equipment`/`run_queue`, `normalize_url`, `connect`  
**Theme:** This is where the campaign's fixes did *not* reach: the CLI is a forked, unvalidated copy of the tool layer, several single-copy fixes never landed here, and `wss://` sync cannot connect at all.

### 🔴 **HIGH** · #31 · `mise fridge add` still allocates portion ids by scanning for the lowest free `p<n>` in the local replica, so concurrent adds on two devices destroy each other on merge

**`crates/cli/src/main.rs:790`** · _bug_

The prior HIGH fix replaced positional ids with `Store::mint_id("p")` only on the assistant tool path (tools.rs:1076-1079), pinned by a tools-level convergence test. The CLI's `FridgeCmd::Add` still does `(1..).map(|n| format!("p{n}")).find(|c| !portions.contains_key(c))` against the local `FridgeDoc`. Because minted ids look like `p-<replica>-<seq>`, the scan never sees them, so on a corpus whose portions came from the assistant or web UI *every* CLI add on every device deterministically picks `p1`. `FridgeDoc.fridge` is an Automerge map, so two concurrent puts at that key resolve to one winner. Lowest-free reuse also survives: after `mise fridge remove p1` the next add re-issues `p1`, so a stale remove from a peer can delete a stranger's portion. Reproduced end to end against two real Stores driven through `run_sync`: after merge the fridge held one portion where two were added. remediation.md:63 records the class as fixed.

- **Spec:** crates/store/src/store.rs:479-486 — positional ids "collide across replicas, where the merge resolves both puts to one winner and the other item silently vanishes; and lowest-free reuse lets a stale remove delete a stranger"; CLAUDE.md, CRDT convergence, concretely.
- **Prior:** still-open
- **Suggested fix:** Mint before the `modify` closure exactly as `tools::fridge_add` does (`let id = match id { Some(id) => id, None => store.mint_id("p")? };`) and drop or slug-restrict the `--id` escape hatch. Better still, route CLI fridge/pantry mutations through `tools::execute`. Add a CLI-level partition test asserting both portions survive the merge.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Verified line by line, and no guard exists.

1. The scan is present verbatim at crates/cli/src/main.rs:790-795 inside the `store.modify::<FridgeDoc>` closure: `id.clone().unwrap_or_else(|| (1..).map(|n| format!("p{n}")).find(|c| !portions.contains_key(c)).expect("unbounded candidate ids"))`. It is purely a local-replica scan of the just-loaded `FridgeDoc`; nothing consults the store counter.

2. `grep -rn "mint_id" crates/` returns only tools.rs:1079 (`fridge_add`), tools.rs:1219 (`shop_add`), store.rs:487 (the definition), and three store test call sites. The CLI never calls it. Correspondingly `grep` for the positional scan finds exactly one remaining site, crates/cli/src/main.rs:792 — so the CLI is the sole unremediated path, confirming the prior fix landed only on the assistant tool path.

3. The contrast is explicit at crates/assistant/src/tools.rs:1076-1079, which mints *before* the closure with a comment naming this exact hazard: "Minted, never positional: portion ids are CRDT map keys, and two replicas both picking the lowest free `p1` while apart merge to one surviving portion."

4. The spec rule is real at crates/store/src/store.rs:479-486: positional ids "collide across replicas, where the merge resolves both puts to one winner and the other item silently vanishes; and lowest-free reuse lets a stale remove delete a stranger."

5. The data shape supports the failure. crates/store/src/pages.rs:280-281 — `pub fridge: BTreeMap<String, PortionDoc>` and `pub freezers: BTreeMap<String, BTreeMap<String, PortionDoc>>` — both map to Automerge maps keyed by portion id, so concurrent puts at key `p1` resolve to a single winner.

6. Multi-replica is not hypothetical for the CLI: `Cmd::Sync` exists at crates/cli/src/main.rs:55-56 and is dispatched at :404, so two devices running `mise fridge add` offline and then syncing is the ordinary usage path.

Two aggravating details also check out. Because minted ids take the form `p-<replica>-<seq>` (store.rs:495), `portions.contains_key("p1")` never matches them, so on a corpus seeded by the assistant or web UI every CLI add on every device deterministically picks `p1` — the worst case. Even on a CLI-only corpus the base case still collides (both replicas pick `p1` against an empty fridge). And the removal path at :805-833 does a plain `remove`, so lowest-free reuse means the next add after `mise fridge remove p1` re-issues `p1`, exposing the stale-remove-deletes-a-stranger half of the spec rule.

docs/remediation.md:63 does record the class as closed — "Shopping items and fridge portions get replica-safe ids — #36, #93 *(legacy `s1` keys go inert, never reused)*" — so the doc and the CLI code disagree, which is itself a defect under CLAUDE.md's agreement invariant.

Severity high is right, not overstated: silent data loss with no conflict surfaced, in the CRDT-convergence area CLAUDE.md singles out for extra coverage. The suggested fix (mint before the closure, as tools.rs:1079 does) is correct; note the `--id` escape hatch at :790 lets a user still inject a positional id, so restricting or dropping it belongs with the fix.

</details>

### 🔴 **HIGH** · #52 · The CLI's WebSocket stack is built without any TLS feature, so every `wss://` sync — the documented production topology — fails with "TLS support not compiled in"

**`crates/cli/src/remote.rs:113`** · _bug_

`tokio-tungstenite = "0.27"` is declared with default features only (`connect`, `handshake`); no native-tls, no rustls. `connect_async` on a `wss://` URL then hits the no-TLS `wrap_stream` fallback and returns `Error::Url(UrlError::TlsFeatureNotEnabled)`, and the lockfile confirms no TLS deps are pulled in. Meanwhile `normalize_url` deliberately maps `https://` → `wss://` with a unit test asserting it, and the documented deployment is "Caddy proxies and terminates TLS". `tests/remote.rs` only ever talks to `ws://127.0.0.1:<port>`, so the suite cannot see it. Reproduced with the built binary. The workaround a user reaches for — saving a `ws://` URL for a public host — sends the bearer token in cleartext.

- **Spec:** implementation.md, Server defaults: "`mise-server` binds 127.0.0.1:7920; Caddy proxies and terminates TLS. Client join flow: `mise init --from <url> --token …`"
- **Suggested fix:** Enable `tokio-tungstenite`'s `rustls-tls-webpki-roots` (or native-roots) feature, matching reqwest's existing `rustls-tls`, and add a test that a `wss://` attempt fails on connect/handshake rather than on `TlsFeatureNotEnabled`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Verified end to end.

1. Dependency: `Cargo.toml:33` declares `tokio-tungstenite = "0.27"` with no `features` key, and `crates/cli/Cargo.toml:22` (plus dev-dep line 31) takes it via `.workspace = true` with no added features. `cargo tree -p mise-cli -e features -i tokio-tungstenite@0.27.0` resolves exactly `default` → `connect` → `handshake`/`stream`. No `native-tls`, no `__rustls-tls`. Cargo.lock:2388-2397 lists tokio-tungstenite 0.27's deps as only futures-util/log/tokio/tungstenite — no tokio-rustls or native-tls. (rustls does appear in the lock, but only under hyper-rustls/reqwest; that does not feature-unify into tungstenite, as the tree confirms.)

2. Runtime path: `crates/cli/src/remote.rs:113` calls `connect_async(request)`. In tokio-tungstenite 0.27, `connect_async` → `connect_async_with_config` → `connect.rs:97` `crate::tls::client_async_tls_with_config`, which with no TLS feature and `Connector::Plain` reaches `encryption::plain::wrap_stream` (tls.rs:157-165): `Mode::Tls => Err(Error::Url(UrlError::TlsFeatureNotEnabled))`. tungstenite-0.27 error.rs:262-263 renders that as "TLS support not compiled in". So any `wss://` target can only ever fail (TCP to :443 may succeed, but the upgrade never does).

3. The scenario is reachable, not hypothetical: `normalize_url` (remote.rs:62-84) deliberately rewrites `https://` → `wss://`, and the in-file unit test at remote.rs:207-222 asserts `https://cook.example.com` → `wss://cook.example.com/sync`; the test fixture `remote()` itself uses a `wss://` URL. That normalized value is what `save`/`load` persists and what `session` passes to `connect_async`. The test suite never exercises it because the unit tests stop at string normalization and the integration tests use `ws://127.0.0.1`.

The secondary point is also fair: the natural workaround (saving a `ws://` URL for a public host) sends the `Bearer` token from remote.rs:107-110 in cleartext.

I did not independently re-run the binary, but the feature resolution and the library source make the failure deterministic, so no runtime check is needed. Severity high is appropriate — it breaks the documented Caddy-terminated-TLS deployment for every remote user.

</details>

### 🟠 **MEDIUM** · #32 · `mise pantry set --tier` accepts any well-formed slug without checking the location's shops page, silently erasing the source tier for every dish that needs the item

**`crates/cli/src/main.rs:729`** · _bug_

The prior MEDIUM finding was fixed in the assistant tools only: `tools::resolve_tier` loads `DocId::Shops(loc)` and returns "no tier X at LOC; tiers are: …", pinned by a test. The CLI still does `tier.map(|t| slug(&t))` and writes the result straight into `PantryItemDoc.tier`. Downstream `Readiness::verdict` treats an unknown tier exactly like a missing one — `tiers.iter().position(...)` yields None and short-circuits the whole assessment to `NeedsShopping { tier: None }` — so one typo erases the tier for *every* dish in that trip, on the CLI queue, in `queue_status` and in `/api/queue`. The two surfaces also disagree about identical input. Reproduced with the built binary.

- **Spec:** implementation.md → Tools: an unknown slug comes back as a model-recoverable error, not a silent default.
- **Prior:** still-open
- **Suggested fix:** Give the CLI the same existence check — load `DocId::Shops(loc)` and bail with the same message listing the real tiers — or call `tools::execute("pantry_set", …)` so `resolve_tier` is the one implementation.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced all four legs of the claim and they hold.

1. CLI writes an unvalidated tier. `crates/cli/src/main.rs:730` is exactly `let tier = tier.map(|t| slug(&t)).transpose()?;` — `slug()` only enforces slug *form*, not existence — and :747-749 writes it straight into `PantryItemDoc.tier`. The only store-side check on the way through is `parse_slug` at `crates/store/src/pages.rs:208`, which again is form-only. `resolve_location` is called (:726) so location existence *is* checked, which makes the missing tier check a genuine asymmetry rather than a deliberate "CLI doesn't validate" stance.

2. The assistant path does validate. `crates/assistant/src/tools.rs:181-194` loads `DocId::Shops(loc)` and returns `user("no tier {tier} at {loc}; tiers are: …")`, with a doc comment (:177-180) stating precisely the rationale this finding gives. A repo-wide grep for `resolve_tier` returns only `tools.rs:181` (definition) and call sites `tools.rs:934` and `tools.rs:1214` — nothing in `crates/cli`. So the two surfaces really do disagree on identical input.

3. The downstream blast radius is as described. `crates/core/src/readiness.rs:104-117`: for each shop need, `tier.as_ref().and_then(|id| tiers.iter().position(|t| &t.id == id))`; on `None` it does `return Verdict::NeedsShopping { tier: None }` — an early return out of the whole loop, so a single unmatched tier collapses the trip tier for *every* need, not just the typo'd item. The doc comment at :97-99 confirms this is intended semantics for unknown tiers, which is what makes the silent write the bug rather than the verdict logic.

4. The failure scenario therefore occurs: `--tier twon` is a well-formed slug, passes `slug()`, is persisted, and every queued dish needing that item degrades to "tier unknown" with no diagnostic anywhere.

Severity medium is right: silent data corruption with wide read-side impact, but single-user CLI, recoverable by re-setting the tier.

One scope note (does not change the verdict): `tools.rs:1214` is a second `resolve_tier` call site, suggesting the CLI's shopping-list command may have the same gap. I did not chase that, so I neither widened nor confirmed the finding beyond `pantry set` as written.

</details>

### 🟠 **MEDIUM** · #33 · The CLI hand-reimplements the mutation half of the tool layer instead of calling it, and the two copies have already drifted on ids, tier validation, servings bounds, equipment-note patching and queue upsert semantics

**`crates/cli/src/main.rs:778`** · _architecture_

tools.rs's module doc says the tool set is "the same operations the CLI and HTTP surface expose" with "no privileged side door". The HTTP surface honours this (`/api/edit/{action}` → `tools::execute`); the CLI does not — `run_pantry`, `run_equipment`, `run_fridge`, `run_queue`, `run_recipe`, `run_log` each open `store.modify` with their own validation and change messages, plus byte-identical private copies of `slugify`, `resolve_location`, `parse_tags`, `parse_date`, `must_trim`, `opt_trim`. The prior review's *read*-half duplication was fixed by having the CLI call `views::render_queue_status`, proving the plumbing exists; the write half was left forked and four separate remediation-campaign fixes landed on only one copy.

- **Spec:** crates/assistant/src/tools.rs:1-9 — "The tool set: the same operations the CLI and HTTP surface expose … No privileged side door"; implementation.md → Tools: "Nineteen deterministic operations mirroring the CLI surface".
- **Prior:** still-open
- **Suggested fix:** Have the CLI mutation subcommands build a `ToolCall` and go through `tools::execute` with `ToolCtx { provenance: "cli", .. }`, mapping `Fail::User` onto `bail!`, exactly as `/api/edit` does — or lift the operation bodies into a shared `ops` module both drivers adapt. Where a CLI-only affordance is genuinely needed, keep it but push shared validation into tools.rs helpers. Failing that, add a parity test suite running each operation through both drivers and comparing doc state.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (architecture); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/cli/src/main.rs:385-889,722-830,907-967 (slugify at :911), :890-901 (the read half, already unified); crates/assistant/src/tools.rs:1-9,114-120,126-194,637-647; crates/server/src/api.rs:219-265


Failure scenario: Realized, not hypothetical: remediation Phase 8 marked "Edits change only the fields they name" complete while `mise equipment add wok` still wipes the note, and the same user action is validated through phone/web/assistant and unvalidated through `mise` on the desktop — against the same synced corpus.

</details>

### 🟠 **MEDIUM** · #50 · `mise equipment add <item>` without `--note` blanks the note an earlier call recorded

**`crates/cli/src/main.rs:692`** · _bug_

The CLI does `e.items.insert(item, note.unwrap_or_default())` — an unconditional replace. The prior "`equipment_set` blanks an existing note when `note` is omitted" finding was fixed in `tools::equipment_set`, which now does entry-and-patch with an explicit comment ("only the fields you pass change. An explicit \"\" clears the note; omitting it keeps what's there"). The CLI kept the old semantics, and `mise pantry set` right next to it *does* patch, so the two CLI commands disagree with each other as well as with the tool layer. Reproduced with the built binary.

- **Spec:** implementation.md / remediation Phase 8: "Edits change only the fields they name".
- **Prior:** still-open
- **Suggested fix:** Mirror `tools::equipment_set`: `let entry = e.items.entry(item).or_default(); if let Some(n) = note.as_deref() { *entry = n.trim().to_string(); }`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced all three pointers and every claim in the finding holds.

1. `crates/cli/src/main.rs:690-693` (EquipmentCmd::Add) does an unconditional map replace:
   `e.items.insert(item.to_string(), note.as_deref().map(|n| n.trim().to_string()).unwrap_or_default())`
   There is no guard on `note` being `None` — when the flag is omitted, `unwrap_or_default()` yields `""` and the insert overwrites whatever note the map already held for that key. No earlier read/merge exists in the closure, and `store.modify` just hands the doc to the closure, so nothing upstream preserves the old value. The failure scenario (add with `--note`, then re-add without) provably lands an empty string in `items[item]`, which then flows through `store.export`.

2. `crates/assistant/src/tools.rs:1014-1022` (`equipment_set`) does exactly the entry-and-patch the finding describes, with the comment "only the fields you pass change. An explicit \"\" clears the note; omitting it keeps what's there." So the tool layer and the CLI genuinely disagree on the same underlying doc.

3. `crates/cli/src/main.rs:733-755` (`PantryCmd::Set`) also patches field-by-field (`if let Some(n) = &note { entry.note = opt_trim(n) }`), confirming the sibling-CLI-command inconsistency claim.

Severity medium looks right: silent loss of user-authored data that replicates via export, but only on a re-add of an already-noted item, and the note is recoverable from history. The one mild counterargument — the subcommand is named `add` rather than `set`, so create-or-replace could be read as intended — does not survive contact with the fact that `equipment remove` errors on a missing item while `add` is the only way to touch an existing entry's note, and the tool layer's stated contract covers the same doc. Keeping at medium.

</details>

### 🟠 **MEDIUM** · #56 · Nothing compares CLI and assistant-tool semantics for the same operation, and the remote tests never exercise a TLS URL or a CLI-created portion under partition

**`crates/cli/tests/cli.rs:53`** · _test-gap_

`tests/cli.rs` is one happy-path smoke test plus a not-found test; `tests/remote.rs` covers join/converge/push-only/cut-off but always over `ws://127.0.0.1`. Every CLI/tool divergence is therefore invisible to CI: no test adds a fridge portion through the CLI on two replicas and asserts both survive a merge (the store's convergence properties allocate through `mint_id`, modelling only the fixed path); no test asserts the same operation through `mise <cmd>` and through `tools::execute` leaves identical doc state; and no test touches a `wss://` URL, so a WebSocket stack with no TLS compiled in passes the whole suite.

- **Spec:** CLAUDE.md, CRDT convergence, concretely; The export never lies.
- **Prior:** still-open
- **Suggested fix:** Add (1) a CLI convergence test — two corpora joined from one server, one offline `fridge add` each, sync, assert both dishes survive and exports match; (2) a parity test per shared operation comparing doc state after the CLI arm and the tool arm; (3) a `wss://` connect test asserting the failure is a connect/handshake error, never `TlsFeatureNotEnabled`.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/cli/tests/cli.rs:53-131; crates/cli/tests/remote.rs:106-144; crates/store/tests/sync.rs:709-735


Failure scenario: A green `cargo test` coexists with silent portion loss on merge through `mise fridge add` and with remote sync being impossible over TLS; remediation.md cites `devices_join_edit_offline_and_converge` as proof of CLI-level convergence and that test never creates a portion.

</details>

### ⚪ **LOW** · #35 · CLI ingress skips the hygiene validations every tool ingress performs: servings are unbounded (and may be 0 in the fridge and log), and lead time accepts 0 minutes with an empty step

**`crates/cli/src/main.rs:780`** · _bug_

`bounded_servings` rejects 0 and >999 on every assistant/HTTP path ("an absurd number persisted once syncs to every replica forever"), and `build_lead` rejects zero minutes and an empty act-now step. The CLI checks only `servings == 0` for `recipe add`; `fridge add` and `log add` bound nothing, and `--lead-minutes 0 --lead-step " "` writes a lead that renders as a bare "start now: ". The prior review explicitly asked to bound servings at the tool *and* CLI ingress; only the tool half landed. Consequences are now data quality rather than a panic, since coverage saturates.

- **Spec:** docs/remediation.md:64 — "Coverage saturates; servings are bounded at ingress — #0, #1"; prior review: "Independently bound servings at the tool/CLI ingress."
- **Prior:** still-open
- **Suggested fix:** Expose `bounded_servings`/`build_lead` from a shared crate (or move them to mise_core) and apply them at the CLI's `fridge add`, `log add` and `recipe add` ingress — or fold these commands into `tools::execute`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every pointer and the CLI validations are genuinely absent. `FridgeCmd::Add` (main.rs:780-802) writes the raw `servings: u32` into `PortionDoc` with no check at all; `LogCmd::Add` (main.rs:839-861) only `.context()`s the missing-value case; `RecipeCmd::Add` (main.rs:623-624) rejects 0 but has no upper bound. Meanwhile `bounded_servings` (tools.rs:111-120, 1..=999) gates the assistant's `fridge_add` (tools.rs:1069) and `log_add` (tools.rs:1178), so the two ingresses provably disagree — `mise fridge add "Gruel" --servings 4000000000` persists where the tool path refuses. The lead claim also holds: main.rs:633-636 builds `LeadTimeDoc { minutes, act_now_step: lead_step.unwrap_or_default().trim().to_string() }` with no zero-minute or empty-step rejection, unlike `build_lead` (tools.rs:817-832). One narrow correction: clap's `requires` at main.rs:176-180 does enforce the minutes/step pairing, so only the zero-minutes / whitespace-step half of that sub-claim is open. The prior-review citation checks out (docs/reviews/2026-07-31-codebase-review.md:139-140 asks for "the tool/CLI ingress"; docs/remediation.md:64 marks it done). Severity `low` is correctly stated — coverage saturates so there is no panic, and the CLI is a single trusted user per the project threat model, making this ingress divergence and data hygiene rather than a live fault.

</details>

### ⚪ **LOW** · #51 · `mise queue add` on an existing id overwrites the entry, resetting its `added` date and dropping sibling dishes

**`crates/cli/src/main.rs:447`** · _bug_

The CLI computes the same content-derived id as the tool and then does an unconditional `entries.insert` with `added: today` and `dishes: vec![the one dish]`. `tools::queue_add` was deliberately changed to upsert-as-patch with a comment explaining why: "Age is load-bearing ('21d on the queue' exists so stale entries are noticeable) and a multi-dish entry is a menu — so an existing entry keeps its `added` and its dishes, and only the reason moves." The CLI has no other way to amend an entry, so re-adding is the natural gesture and it silently resets the staleness signal `render_queue_status` prints. The sibling-dish half is latent today but goes live as soon as the assistant composes a menu.

- **Spec:** crates/assistant/src/tools.rs:678-688 — "an existing entry keeps its `added` and its dishes, and only the reason moves. Changing the dish itself is queue_remove + queue_add."
- **Prior:** still-open
- **Suggested fix:** Apply the same entry-and-patch in the CLI's queue-add arm, ideally by extracting the upsert once (`pages::queue_upsert(&mut QueueDoc, id, dish, reason, today)`) called from both `tools::queue_add` and the CLI so the rule cannot drift again.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced both paths and they genuinely diverge.

CLI (`crates/cli/src/main.rs:441-459`): computes the same id (explicit `--id` slug, else `slug(&slugify(&title))`), then unconditionally `q.entries.insert(id.to_string(), QueueEntryDoc { dishes: vec![one dish], reason, added: today.to_string() })`. `HashMap`/`BTreeMap::insert` replaces the existing value wholesale, so an existing entry's `added` is reset to today and any sibling dishes in the entry's `dishes` vec are discarded. There is no `get_mut`/contains-key guard anywhere in that arm, and no other CLI subcommand amends an entry — `QueueCmd` only has Add/Remove in this region (`main.rs:464-479`).

Tool (`crates/assistant/src/tools.rs:674-704`): explicitly matches `q.entries.get_mut(&id.to_string())`; on `Some` it only moves `reason` and leaves `added`/`dishes` intact, with the comment at 678-682 stating the rule ("an existing entry keeps its `added` and its dishes, and only the reason moves"), and returns "kept its place, age, and dishes". So the spec_rule quoted in the finding is verbatim in the code.

The staleness signal it erases is real: `crates/assistant/src/views.rs:248` renders `", {days}d on the queue"` from `added`, and the CLI's own `show_queue` calls `views::render_queue_status` (`main.rs:900`). So `mise queue add` on an existing id both resets the displayed age and (once multi-dish menu entries exist) drops siblings, exactly as the failure_scenario describes.

Severity `low` is fair — personal-app, data loss limited to the `added` date today — though the sibling-dish drop would be worse once menus are composed.

</details>

### ⚪ **LOW** · #53 · `normalize_url` panics on a scheme-only URL because the trailing-slash trim invalidates the `expect("checked above")` invariant

**`crates/cli/src/remote.rs:78`** · _bug_

The scheme check runs before `url.trim_end_matches('/')`, and the trim removes the `//` that the later `split_once("://")` relies on, so `"ws://"` → `"ws:"` → None → panic. Same for `"http://"`/`"https://"`, which normalize to `ws://`/`wss://` first. A bare `expect` message is a poor failure mode for a typo when every other malformed URL gets the friendly "server URL must start with …". Separately, a URL with a query but no path (`https://host?x=1`) yields the malformed `wss://host?x=1/sync`, since `contains('/')` is the only path test.

- **Suggested fix:** Split the scheme off once up front, validate the remainder is non-empty before trimming, then decide the default path from it. Extend the normalize table test with `ws://`, `http://` and a query-string case.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced `normalize_url` at crates/cli/src/remote.rs:62-84 and the code matches the finding exactly.

Panic path: for input `"ws://"` the scheme guard at :68 (`url.starts_with("ws://")`) passes, so `url` becomes `"ws://"`. Line 77 `url.trim_end_matches('/')` strips *all* trailing slashes, yielding `"ws:"`. Line 78 `url.split_once("://")` then returns `None` and `.expect("checked above")` panics. Identical for `"http://"` (strip_prefix leaves `rest = ""` → `format!("ws://")` → `"ws://"`) and `"https://"` → `"wss://"`. The scheme validation at :64-72 runs before the trim, so the "checked above" invariant is genuinely invalidated by the trim — the comment at :73-76 explains the trim's ordering rationale but never considered the empty-authority case.

No upstream guard exists: the only callers are crates/cli/src/main.rs:330, :391, :407, and each passes the raw clap-parsed string straight in with no prior URL validation, so `mise remote set ws://` / `mise init --from http://` do reach the panic. The failure scenario is reproducible as described.

Secondary claim also holds: `after_scheme.contains('/')` at :79 is the only path test, so `"https://host?x=1"` → `"wss://host?x=1"` (trim is a no-op, no `/` present) → `"wss://host?x=1/sync"`, where the `/sync` lands inside the query string rather than as a path. That value is then *saved* to remote.json (main.rs:341), so the mistake persists across runs.

The existing table test at :208-222 covers only well-formed host URLs plus one bad-scheme case, so none of these inputs are exercised.

Severity "low" is right: it takes a malformed hand-typed URL to trigger, and the outcome is a confusing panic message instead of the friendly scheme error (plus a silently wrong saved URL in the query case) — not data loss or a security issue.

</details>

### ⚪ **LOW** · #54 · A sync session has no deadline or keepalive, so a blackholed connection hangs `mise sync` indefinitely

**`crates/cli/src/remote.rs:123`** · _bug_

`session()` awaits `ws.next()` in an unbounded loop with no read timeout, no session deadline and no ping/pong; the server side has none either. A half-open TCP connection — a laptop changing networks mid-sync, a NAT dropping state, a proxy blackholing — leaves the process waiting forever with no output and no exit. The model client was hardened against exactly this class during the remediation campaign; the sync client never got the equivalent, and `a_sync_cut_off_early_fails_instead_of_reporting_success` covers only a clean close.

- **Spec:** implementation.md, Anthropic client: "Connect and between-chunk read timeouts, so a blackholed connection fails instead of hanging the stream" — the standard the sync transport does not meet.
- **Suggested fix:** Wrap the session future in `tokio::time::timeout` (a whole-session budget) and/or apply a per-message read timeout, so a stalled peer takes the existing "already-received data is saved; run `mise sync` again" error path.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and the finding holds as written. `session()` (crates/cli/src/remote.rs:106-144) does `connect_async(request).await` with no connect timeout (:113) and then `while let Some(incoming) = ws.next().await` (:123) with no per-read timeout, no whole-session deadline, and no ping/pong keepalive; nothing wraps the future in `tokio::time::timeout` at the `sync()` call site either (:91-104, plain `runtime.block_on`). A grep for `timeout|Ping|WebSocketConfig` across crates/ shows the only network timeouts in the repo are in the model client (crates/assistant/src/client.rs:41-42, `connect_timeout` + `read_timeout`) and the URL fetcher (crates/assistant/src/fetch.rs:63,169) — the sync transport got none, which matches the finding's contrast with the remediation-campaign hardening and with implementation.md:181 ("Connect and between-chunk read timeouts, so a blackholed connection fails instead of hanging the stream"). The server side is likewise bare: `handle_sync` loops on `socket.recv().await` with no deadline and never sends pings (crates/server/src/lib.rs:244-272), so a half-open TCP connection leaves both ends parked. With a silently blackholed socket (network change / NAT drop) neither `next()` nor the OS produces an error, so `mise sync` blocks forever rather than taking the `bail!("sync ended early — already-received data is saved; run `mise sync` again")` path at :141. Severity "low" is reasonable: no data loss or corruption (each round is persisted before reply), the failure is a hang requiring a manual kill on an uncommon network event.

</details>

### ⚪ **LOW** · #55 · CLI change messages interpolate free-text user input raw and unbounded into immutable, replicating page history

**`crates/cli/src/main.rs:784`** · _quality_

`ToolCtx::msg` was hardened into the one funnel for provenance — control characters and whitespace collapse to single spaces, capped at 200 characters — precisely because change messages are immutable and replicate to every device. The CLI builds its own with `format!("cli: fridge {loc}: add {dish}")`, `format!("cli: log {}", entry.title)` and the chat summary, where `must_trim` only trims the outer edges: interior newlines survive and length is unbounded. Under the project threat model this is not an exploit, but it is a second unnormalized funnel into the same immutable history the first one guards.

- **Spec:** prior review, "Provenance messages interpolate model text unescaped and unbounded" — "One rule at the funnel, whatever the source" (tools.rs:48-51).
- **Prior:** still-open
- **Suggested fix:** Move the normalization out of `ToolCtx::msg` into a shared helper (store or core) and route the CLI's message construction through it, so there is one rule at the funnel regardless of source.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: crates/cli/src/main.rs:595-605,784,865; crates/assistant/src/tools.rs:47-65


Failure scenario: `mise fridge add "$(pbpaste)"` with a multi-line snippet on the clipboard writes a multi-kilobyte, multi-line entry into the fridge page's Automerge history and the export's git log — where the same text through the assistant would have been one 200-char line.

</details>

---

## Web client

**Files:** `web/src/lib/*.ts`, `web/src/lib/components/*.svelte`, `web/src/routes/**/*.svelte`  
**Read first:** implementation.md → the 2026-07-31 decisions (taps change data not structure, one representation at a time, proposal lifecycle, composer contract)  
**Key entry points:** `safeUrl`, `Thread.svelte:send`, the recipe-status side fetch, the token gate, `ReconProposal`  
**Theme:** The XSS raw-HTML half is fixed; the URL half is bypassable by entity encoding, and several error paths erase their own banner, the user's draft, or the whole rendered page.

### 🟠 **MEDIUM** · #57 · `safeUrl`'s scheme allowlist is bypassed by HTML character references, because it tests the raw markdown text while the browser decodes references in the href attribute it emits

**`web/src/lib/markdown.ts:43`** · _security_

`safeUrl` normalizes only characters at or below U+0020, matches `HAS_SCHEME = /^([a-z][a-z0-9+.-]*:)/i`, and on no match returns the **original** string. marked's only href post-processing is `cleanUrl()` = `encodeURI()`, which leaves `&`, `#` and `;` intact, and `Markdown.svelte:43` injects the result with `{@html}`, so the HTML parser decodes references inside the attribute value. Verified against the repo's own marked 18.0.7 with this exact config: `javascript&colon;`, `javascript&#58;`, `javascript&#x3a;`, `&#106;avascript:`, `&NewLine;javascript:` and `java&Tab;script:` all reach the href unchanged, on links, images and reference-style definitions. The `&Tab;` case is sharpest: the control-character filter exists precisely to catch `java<TAB>script:`, and the entity spelling walks past it. Raw-HTML escaping is sound; this is specifically the URL half. What saves the app today is the build's CSP (`script-src 'self' 'sha256-…'`, verified present in web/build/index.html), which blocks `javascript:` navigation — so the primary control is void and only the depth layer holds, and schemes CSP does not govern (custom app schemes, `file:`, `intent:` on the Android PWA) are not covered at all.

- **Spec:** markdown.ts:33-42 — "A URL carrying a scheme must carry one of ours … an allowlist means a scheme we have not thought about cannot surprise us either"; Markdown.svelte:40-42 — "renderMarkdown … refuses executable URL schemes."
- **Suggested fix:** Decode HTML character references (numeric plus the named ones browsers accept in attributes: `&colon;`, `&Tab;`, `&NewLine;`) before the scheme test, and return the decoded, control-stripped string rather than the original `href`, so what is emitted is what was validated. Additionally escape `&` to `&amp;` on output so a reference can never re-form in the attribute. Add the entity spellings to `markdown.test.ts`'s refusal table for links, images and reference definitions, and stop the `Markdown.svelte:40-42` comment from claiming a guarantee the code does not provide.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Reproduced against the repo's own marked 18.0.7 using the exact Marked config from web/src/lib/markdown.ts. Every entity spelling the finding lists reaches the emitted attribute unchanged: `[a](javascript&colon;alert(1))` -> `<a href="javascript&colon;alert(1)">`, likewise `&#58;`, `&#x3a;`, `&#106;avascript:`, `&NewLine;javascript:`, `java&Tab;script:`, on links, images (`<img src="&#106;avascript:...">`) and reference-style definitions. Only the plain `javascript:` spelling is neutralized (`<a href="">`).

Mechanism traced end to end: safeUrl (markdown.ts:44-46) filters only characters at or below U+0020, tests HAS_SCHEME = /^([a-z][a-z0-9+.-]*:)/i (:31,:47), and on no match returns the ORIGINAL href at :49 rather than the normalized `bare`. marked's only href post-processing is V() = encodeURI(l).replace(percentDecode,"%"), and the link/image renderers emit `'<a href="' + e + '"'` / `` `<img src="${e}"` `` with no attribute escaping (marked.esm.js:75) — encodeURI leaves `&`, `#`, `;` intact. Markdown.svelte:43 injects with {@html}, so the HTML parser decodes the character reference inside the attribute value, yielding a live `javascript:` URL.

The `java&Tab;script:` case confirms the finding's sharpest point: the <=0x20 filter exists specifically to catch `java<TAB>script:` (markdown.test.ts:62 tests the literal tab) and the entity spelling walks past it.

CSP claim also checked and accurate: web/svelte.config.js:15-29 sets csp mode 'hash' with 'script-src': ['self'], which does block javascript: navigation today — so the app is not presently exploitable for script execution, and the finding says so explicitly. That leaves the documented control (markdown.ts:33-42 and the Markdown.svelte:40-42 comment claiming renderMarkdown "refuses executable URL schemes") void, with only the depth layer holding, and schemes CSP does not govern are uncovered. Medium is the right severity: a real bypass of the documented trust boundary, currently backstopped only incidentally.

</details>

### 🟠 **MEDIUM** · #60 · An exchange failure reported over SSE as an `error` event is written into `error` and immediately erased by the `reload()` on the next line, so the thread's error banner never renders for any server-side failure

**`web/src/lib/components/Thread.svelte:107`** · _bug_

`chat()` resolves normally when the server ends the stream after an error frame, so `send` runs `onError → error = message`, then `await reload()`, whose first statement is `error = null`. The banner is cleared in the same tick it was set. On the server `drive` sends `error` and never `done`, and `exchange` sends `done` only on success, so the two are mutually exclusive and every server-side exchange failure takes this path. When the failure happened *before* the user turn was appended — `ThreadId::parse` failure, `store.exists` rejecting the page, `recon::validate_all` rejecting the photos — no marker is written either, so the optimistic bubble is replaced by the unchanged transcript and the composer has already consumed the draft and photos because `send` resolved. The sibling `cookbook/+page.svelte` gets this right by accident: its `reload()` does not touch `error`.

- **Spec:** implementation.md, The web app — the thread surfaces exchange failures; the prior review's recurring "error paths abandoning state" root cause.
- **Suggested fix:** Track the streamed error separately (`let streamError: string | null` set from `onError`) and re-apply it after `await reload()`; or stop `reload()` from clearing `error` and clear it explicitly at the top of `send()` and in the thread-change effect. Add an e2e case scripting the fake to emit an `error` frame and asserting the banner is visible (today's tests only use `route.abort()`, which takes the rejecting path).

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every link in the chain and it holds.

1. `chat()` (web/src/lib/api.ts:115-153) resolves normally on an `error` frame — line 145 just calls `events.onError(...)`; there is no throw and no rethrow anywhere in the read loop. Only a non-ok HTTP status (api.ts:51-59) or a network/abort failure rejects. So an SSE-reported exchange failure returns control to `send()` on the success path.

2. `Thread.svelte:101` sets `error = message` from `onError`; line 106 passes the `live()` guard (same generation — nothing bumped `exchange`); line 107 `await reload()`, and `reload()`'s last statement is `error = null` (Thread.svelte:40). The banner at line 146-148 is therefore cleared in the same exchange it was set. `onExchangeDone?.()` then fires as if the exchange succeeded.

3. Server-side mutual exclusivity is as described: `drive` (chat.rs:35-41) emits `error` only when `exchange` returns `Err`, and `exchange` emits `done` (chat.rs:196) only on the `Ok` path, after which it returns `Ok(())`. So every server-side failure takes the error frame and never a `done`.

4. The pre-append cases are real and worse: `ThreadId::parse(p)?` (chat.rs:55), `recon::validate_all(...)` (chat.rs:63), and the `store.exists` rejection (chat.rs:71-73) all return before `append_thread_message` at chat.rs:86, so no `(no reply — the exchange failed: …)` marker is written either (that marker is only produced at chat.rs:184-189, on the post-append path). `reload()` then overwrites `messages` with the unchanged server transcript, dropping the optimistic bubble from Thread.svelte:92.

5. The draft/photos really are consumed: `Composer.submit` (Composer.svelte:41-49) clears `draft` and `files` up front and restores them only in `catch` — and `send()` resolved, so no catch. The MAX_TOTAL photo scenario in the finding is exactly reachable.

6. The contrast with `cookbook/+page.svelte` is accurate: its `reload()` (lines 19-21) only assigns `pages` and never touches `error`, so the drafting box's banner survives.

Severity medium is right — the common mid-exchange failure still leaves a visible failure marker in the reloaded transcript, so the fully silent loss is confined to the three pre-append rejections; but the error banner is dead for all server-side failures.

</details>

### 🟠 **MEDIUM** · #61 · The camera — and therefore the whole recon apply path — is offered on every location's pantry page, including non-active locations where the editor is deliberately hidden, and a proposal without a `location` field applies its taps to the active pantry

**`web/src/routes/page/[...path]/+page.svelte:143`** · _spec-drift_

The Edit toggle and editors are correctly gated on `editable = editorLocation.location === activeLocation`, but `photos={editorLocation?.kind === 'pantry'}` is not. On a foreign location's pantry page the user gets a camera button and a `ReconProposal` card with live Apply buttons. If the model sets `proposal.location` the tap edits a non-active location through a back door the gate was meant to close; if it omits it (optional in the schema and never defaulted by `parse_proposal`), `ReconProposal` omits it too and the server's `resolve_location` falls back to the *active* location — so a tap in front of the cabin shelf silently rewrites the home pantry, with nothing on screen naming which pantry was touched.

- **Spec:** implementation.md: "Editors on a non-active location's page stay hidden until the M8 location selector"; Taps change data, never structure — a tap's effect must follow from what is on screen.
- **Suggested fix:** Gate the camera on the same predicate: `photos={editable && editorLocation?.kind === 'pantry'}`. Independently, have `ReconProposal` always send an explicit location (falling back to the page's), or make `location` required in `propose_pantry_diff`'s schema with `chat.rs` rejecting a mismatch against the thread's page. Pin with an e2e case on a second, non-active location.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/src/routes/page/[...path]/+page.svelte:70-72,121,130,143; web/src/lib/components/ReconProposal.svelte:44-49; crates/assistant/src/recon.rs:141-144; crates/assistant/src/tools.rs:352,367


Failure scenario: On /page/locations/cabin/pantry (cabin not active) the user snaps the cabin shelf; the model returns a line for `rice: out` with no `location`. ReconProposal posts `/api/edit/pantry-set` without a location, the server resolves to `home`, and rice is marked out at home. The cabin pantry is unchanged, the home pantry is wrong, and home's readiness math is wrong with it.

</details>

### 🟠 **MEDIUM** · #62 · The recipe-status side fetch routes its failures into the page-level `error`, which is an exclusive template branch, so a failed `/api/pages` replaces an already-rendered page with an error article

**`web/src/routes/page/[...path]/+page.svelte:63`** · _bug_

The prior review asked for a `.catch` on this fetch; the catch that was added writes into `error`, the same variable the template branches on exclusively (`{#if error} … {:else if content !== null} …`). `/api/pages` walks the whole corpus and is the slower of the page's two fetches, so the ordinary sequence is: the page paints, then `api.pages()` rejects, and the entire page — rendered content, editors, Recent changes, and the Thread with its composer — is replaced by a bare `⚠` banner. The status row is a decoration already hidden when `status` is null; losing it should cost the row, not the recipe and the thread. Nothing clears `error` afterwards on this path, since `reload()` is the only writer of `error = null` and does not re-run. The sibling fetch added in the same campaign degrades correctly (`api.location().catch(() => (activeLocation = null))`). `setStatus`'s catch has the same effect but at least follows a user action.

- **Spec:** implementation.md, Taps change data, never structure / design doc, Graceful decay: "slightly stale suggestions, not a broken database demanding reconciliation."
- **Prior:** still-open
- **Suggested fix:** Give the status fetch its own local failure state: `.catch(() => (status = null))`. If a banner is wanted, render it above the content rather than in place of it. Longer term, return the status on `/api/page/{path}` so the page needs one fetch.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced exactly as described. `web/src/routes/page/[...path]/+page.svelte:57-63` is the recipe-status side fetch: `api.pages().then(...).catch((e) => (error = String(e)))`. `error` is the page-level state declared at line 15, and the template at line 105 branches on it exclusively: `{#if error}<article><p>⚠ {error}</p></article>{:else if content !== null} … {/if}`. So a rejected `/api/pages` blanks the rendered Markdown, the pantry/equipment editors, `History`, and the `Thread` composer, replacing them with a bare banner — the status row is otherwise purely decorative and is already conditionally hidden by `{#if recipeSlug && status}` at line 108, so a failure should have cost only that row.

`api.pages()` does reject on any non-2xx: `web/src/lib/api.ts:69` maps to `getJson('/api/pages')` and `request` throws `new Error(detail || ...)` at line 58 (plus fetch's own network rejection), so the 500-from-transient-store-lock scenario is reachable.

Recovery claim also checks out: the only writer of `error = null` is `reload()` at line 90, called from the path effect (line 97) and from the `onChanged` / `onReverted` / `setStatus` paths — and every one of those callers lives inside the `{:else if content !== null}` branch that the error banner has just replaced, so nothing on screen can trigger it. Only a navigation re-runs the effect. The sibling fetch in the same effect block degrades correctly by contrast (`api.location().catch(() => (activeLocation = null))`, line 101), which confirms the intended pattern.

One nuance that does not refute the finding: both effects start concurrently on mount, so if `api.page(path)` happened to resolve *after* the `pages()` rejection, `reload()`'s `error = null` would clear the banner. But `/api/pages` is the corpus-wide listing and the slower of the two, so the ordinary ordering is content-first — and even in the lucky ordering the bug is a race, not a guard. Medium severity is appropriate: recipe pages only, and it takes a backend failure to trigger.

</details>

### 🟠 **MEDIUM** · #71 · Nothing in any test suite asserts the built page carries its Content-Security-Policy meta tag, even though that meta is the entire script-src/object-src/base-uri policy and the last line of defence behind the `{@html}` sink

**`web/svelte.config.js:15`** · _test-gap_

The CSP is split deliberately: svelte.config.js emits `default-src`/`script-src`/`object-src`/`base-uri`/`connect-src`/`img-src` into a build-time meta tag, and lib.rs adds only the header-only parts. Only the second half is tested — `crates/server/tests/auth.rs:139-153` asserts the three server-added headers against its own hand-written `index.html` fixture, so by construction it cannot see the meta. A repo-wide grep finds no other CSP assertion; the Playwright specs run against the real build but never read the document's CSP, and would boot identically with no policy at all. Given that CSP is currently the only thing standing between an entity-encoded `javascript:` href and `localStorage['mise-token']`, the coverage is missing exactly where the risk is concentrated. A build-tool config option that silently emits nothing when misconfigured is not correct by construction.

- **Spec:** implementation.md:232-235 — "CSP: the build carries its own policy as a meta tag … and the server adds the header-only parts on static responses."
- **Suggested fix:** Add one Playwright assertion (the e2e suite already serves the real build through mise-server): read `meta[http-equiv="content-security-policy" i]`'s content and assert it contains `script-src 'self'` with a `sha256-` hash, `object-src 'none'` and `base-uri 'self'`, and no `unsafe-inline` in script-src. Keep the existing header test — the two halves are separate controls.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/svelte.config.js:15-33; web/build/index.html; crates/server/tests/auth.rs:139-153; web/e2e/*.spec.ts; web/src/lib/markdown.ts:7


Failure scenario: Delete the `csp` block (plausible while chasing an inline-bootstrap hash mismatch after a SvelteKit bump) and rebuild: `npm test`, `npm run e2e` and `cargo test` all stay green while the app ships with no script-src, and the entity-encoded `javascript:` bypass becomes live bearer-token theft on the first recipe drafted from a hostile page.

</details>

### ⚪ **LOW** · #58 · The URL-refusal tests only cover literal scheme spellings and assert with substring checks, which is why the character-reference bypass survived

**`web/src/lib/markdown.test.ts:59`** · _test-gap_

The file's own doc comment argues substring assertions are the wrong tool and the raw-HTML block correctly uses `tagsIn()`; the URL block does the opposite, with `.not.toContain('javascript:')` and a `safeUrl` table enumerating only literal spellings. Both styles pass unchanged on `&#106;avascript:...`, since neither the rendered output nor `safeUrl`'s return value contains the substring — the decoding happens in the browser's HTML parser, which the unit test never runs. This is the highest-risk invariant in the web client.

- **Spec:** CLAUDE.md, Testing: "High-risk areas get extra coverage"; the file's own header on asserting element names rather than substrings.
- **Suggested fix:** Assert on the resolved href: parse `renderMarkdown(...)` with jsdom/happy-dom under vitest and check `a.protocol` / `img.src` against the allowlist, which runs the same entity decoding the browser does. Extend the refusal table with `&#106;avascript:`, `&#x6A;avascript:`, `&NewLine;javascript:`, `&Tab;javascript:` and the reference-definition form.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/src/lib/markdown.test.ts:52,56,59-68


Failure scenario: `npm test` passes all six refusal cases and both `javascript:` render cases while `renderMarkdown('[x](&#106;avascript:alert(1))')` emits a working javascript: anchor — the suite reports the sanitizer correct on the one spelling that defeats it.

</details>

### ⚪ **LOW** · #59 · The TS SSE framer does not mirror the Rust one on the two axes the Rust framer explicitly documents: line-ending agnosticism and a buffer cap

**`web/src/lib/sse.ts:16`** · _spec-drift_

client.rs:258-262 documents its framer as line-ending-agnostic (LF, CRLF, lone CR) because the client is designed to be pointed at proxies and fakes, and caps the buffer at 16 MiB. The TS framer does neither: `indexOf('\n\n')` recognises only the LF-LF separator and `split('\n')` only LF breaks, so a CRLF-terminated stream never yields a frame — the buffer grows unbounded and `chat()` produces no deltas, no done and no error, resolving as a successful-looking empty exchange. Not currently reachable in production (axum writes LF and Caddy proxies bytes unchanged), but reachable the moment a different proxy or fake is used, which is exactly why the Rust side is written the way it is. `sse.test.ts` feeds LF only.

- **Spec:** implementation.md, The web app: "Chat streams over fetch with a TS SSE framer mirroring the Rust one, vitest-covered."
- **Suggested fix:** Normalize line endings on ingest (`chunk.replace(/\r\n?/g, '\n')`, holding a trailing lone `\r` across chunk boundaries as the Rust `finish()` does) and add a buffer cap that throws. Extend `sse.test.ts` with CRLF and lone-CR variants, mirroring client.rs:555-575.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/src/lib/sse.ts:12-21; crates/assistant/src/client.rs:255-315; web/src/lib/sse.test.ts:5-19


Failure scenario: Behind an intermediary that re-emits SSE with CRLF, `SseFrames.push` finds no `\n\n` and keeps buffering; `chat()` reads to EOF, fires no callback, and resolves normally — the composer clears, the assistant bubble stays empty, and no error is shown.

</details>

### ⚪ **LOW** · #63 · Navigating between two doc pages reuses the component instances and leaves the previous page's content, history and transcript on screen while the new data loads — with the action buttons already rebound to the new doc

**`web/src/routes/page/[...path]/+page.svelte:84`** · _bug_

`/page/[...path]` is one route, so params update in place: `doc`, `recipeSlug` and `editorLocation` are `$derived` and flip synchronously, while `content`, `History.changes` and `Thread.messages` are overwritten only after their awaits resolve. During the load window the screen shows page A's markdown, change list and transcript while every control acts on page B. History is the sharp edge — its rows render page A's timestamps and messages, and its buttons call `api.revert(doc, hash)` with page B's `doc`. Thread's effect deliberately leaves `messages`, so page A's conversation sits under page B's heading with an enabled composer wired to B; if the new `reload()` fails, `messages` is never replaced at all. The "stale content stays visible" rule in the spec is scoped to *edits*, where staleness means an older version of the same thing.

- **Spec:** implementation.md, Taps change data, never structure — components reload in place and stale content stays visible until new data lands (a rule about edits, not navigation); a tap's effect must follow from what is on screen.
- **Suggested fix:** Reset per-target state at the top of the path/doc effects rather than only on success: `content = null` in the path effect (the `{:else}` already renders Loading…), `messages = []` in Thread's thread-change effect, and clear `changes` in History when `doc` changes but not when only `version` bumps. Alternatively gate the action controls on a loaded-for token.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The mechanism the finding describes is exactly what the code does. In web/src/routes/page/[...path]/+page.svelte the path effect (lines 94-102) calls `reload()`, which assigns `content` only after the await resolves (line 86) — it never clears `content` first, unlike the recipe-status effect right above it which does `status = null` at entry (line 53). Since `/page/[...path]` is a single route, SvelteKit updates params in place: `doc`/`editorLocation`/`recipeSlug` are `$derived` and flip synchronously, while the `{#if content !== null}` branch (line 107) keeps rendering page A's Markdown, plus `<History {doc} …>` and `<Thread thread={doc} …>` with the *new* doc props.

History.svelte reloads on `doc`/`version` change (lines 22-26) but leaves `changes` populated until the fetch resolves, and `revert(hash)` closes over the prop `doc` (line 30), so a row rendered from page A calls `api.revert(newDoc, oldHash)`. Thread.svelte's thread-change effect (lines 43-57) deliberately resets `streaming`, `toolNotes`, `error`, `proposal` and `busy` but *not* `messages`, and `reload()` only overwrites `messages` on success (line 34) — so page A's transcript sits under page B with a composer whose `send` posts to the new `thread` (line 94), and if reload throws, `messages` is never replaced.

One correction to impact: the destructive part of the failure scenario is blocked server-side. crates/store/src/store.rs:590 rejects a hash not present in the target doc (`StoreError::NotFound`), and crates/server/src/api.rs:208-211 turns that into a failure response, so the cross-doc revert errors out rather than reverting the wrong document (hashes are content-derived per-doc, so a collision is not realistic). What remains is a genuine but non-corrupting UI defect: stale content/history/transcript displayed for the new URL, controls bound to the new doc, and a confusing error on the revert tap. Reachability also requires an in-place page→page navigation (a markdown-body link or back/forward between two /page/* entries); via /browse the component remounts. Hence low rather than medium.

</details>

### ⚪ **LOW** · #64 · A transport failure after the server has already appended the user turn is treated identically to one before it, so the restored draft plus a natural retry writes the same turn to the thread twice

**`web/src/lib/components/Thread.svelte:115`** · _bug_

`chat.rs` appends the user message before the first model call, so anything breaking the stream after that point rejects out of `chat()`. `send`'s catch removes the optimistic bubble, sets `error` and rethrows so Composer restores the draft and photos — right for the pre-append case (what `composer.spec.ts` tests via `route.abort()`), wrong for the post-append case where the message *is* on the thread. The screen reads unambiguously as "it didn't go", so the user re-sends and the append-only transcript carries the turn twice, with the photos re-uploaded. The client cannot distinguish the cases today because the server sends nothing marking "your turn is recorded" — a design gap, in the bad-signal environment the design names as motivating.

- **Spec:** implementation.md, The chat composer is a textarea: "a send that rejects puts the message and any attached photos back in the box"; design doc:448 (store mode's offline tolerance).
- **Suggested fix:** Have the server emit a cheap `accepted` frame carrying the `asked` stamp right after `append_thread_message`; on a rejection after `accepted`, keep the optimistic bubble, do not rethrow into the composer, reload the thread and show the error. Failing that, reload the thread on the failure path before restoring the draft so the transcript shows what actually landed.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced every link in the chain and the code says what the finding claims.

1. Server appends before streaming: `crates/server/src/chat.rs:86` — `store.append_thread_message(&thread, Role::User, &stored, asked)?` runs inside the pre-model lock block (lines 68-91), before `Turn::new` and the first `client.next_turn(...)` at line 100. So any failure after that point leaves the user turn persisted on the thread.

2. Client cannot tell: `web/src/lib/api.ts:115-153` — `chat()` posts, then loops on `reader.read()`. The only frames it understands are `delta`, `tool`, `proposal`, `done`, `error`. There is no "accepted"/receipt frame, and no idempotency key on the POST (`request()` at api.ts:35-61 adds only the bearer header). A mid-stream socket drop makes `reader.read()` throw, so `chat()` rejects — indistinguishable from a pre-append failure like `route.abort()`.

3. Failure path assumes nothing landed: `web/src/lib/components/Thread.svelte:109-117` — the catch (after the `!live() || ctl.signal.aborted` early return, which does NOT cover a plain transport error) does `messages = messages.filter((m) => m.created !== '')`, sets `error`, and rethrows. There is no `reload()` on this path; `reload()` is called only on success (line 107) and on thread change (line 55). So the transcript on screen loses the bubble and never re-syncs with what the server actually stored.

4. Draft comes back and invites a retry: `web/src/lib/components/Composer.svelte:44-49` — `catch { draft = message; files = sent; }`. Re-sending POSTs again; chat.rs:86 appends a second user turn (`stamp_after` guarantees it sorts later rather than deduping). The thread — and the markdown export derived from it — then carries the question twice.

I found no guard the finding missed: no receipt frame, no client-side dedup, no reload-on-error, no request idempotency. Note the server-`error`-frame case is genuinely fine (stream ends normally, `chat()` resolves, Thread reloads at line 107) — but that is a different case from the transport drop the finding describes.

Severity `low` is fair: it needs a mid-stream connection loss to trigger, and the damage is a duplicated turn in an append-only transcript rather than lost or corrupted state.

</details>

### ⚪ **LOW** · #66 · The token gate stores the candidate token on any non-401 response, so a 500, a 403 or an SPA-fallback 200 dismisses the prompt with an unverified credential

**`web/src/routes/+layout.svelte:25`** · _bug_

`save()` treats "not 401" as "verified", while the comment above states "The gate stores nothing unverified: one fat-fingered character must not dismiss the prompt permanently." If `/api/location` returns 500 the token is stored and the gate opens, so the app mounts and every call fails with a message the user cannot act on. Recovery exists only for a genuinely wrong token (api.ts:43-50 clears and reloads on a real 401); a token stored during an outage stays stored, and a 403 would never loop back.

- **Spec:** implementation.md, The web app: "One token prompt, localStorage, 401 loops back to it"; +layout.svelte:11 — "The gate stores nothing unverified."
- **Suggested fix:** Accept only `r.ok` and treat everything else as "could not verify", distinguishing 401 ("the server refused that token") from other statuses ("the server is not answering right now") in `gateError`.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** The code says exactly what the finding claims. In `save()` the only checked status is `r.status === 401` (line 25); the early `return` there is the sole guard, so any other HTTP outcome — 500, 502, 403, or an SPA-fallback 200 — reaches `setToken(candidate); token = candidate;` at lines 29-30 and opens the gate with a credential that was never verified. Only a thrown fetch (network failure, CORS) is caught at line 31. This directly contradicts the stated invariant in the comment at line 11 ("The gate stores nothing unverified: one fat-fingered character must not dismiss the prompt permanently"). The suggested fix (accept only `r.ok`) is the right shape. Minor overstatement: `api.ts:43-50` does clear the token and reload on a genuine 401, so a wrong token stored during an outage will eventually bounce the user back to the prompt once the server is reachable — but the finding's own failure_scenario concedes this ("the prompt does not return until a call actually reaches the server"), and a proxy 403 would never produce that 401. Severity `low` is appropriate: the app is single-user with a static bearer token, and the damage is a confusing intermediate state, not a security bypass (the server still rejects the bad token on every real call).

</details>

### ⚪ **LOW** · #67 · The failed-turn cleanup in the drafting box removes every user turn whose text equals the failed one, not just the turn it pushed

**`web/src/routes/cookbook/+page.svelte:64`** · _bug_

`draftRecipe` pushes `{role:'user', content: what}` and on failure removes turns by matching role and content. Multi-turn refinement is the box's stated purpose and short refinements repeat naturally ("spicier", "again"), so a failed repeat erases the earlier successful turn too, taking the assistant's reply between them out of context. Display-only — the drafting thread is intact server-side — which is why it is low. `Thread.svelte` avoids this by filtering on `m.created !== ''`, a structural marker.

- **Spec:** implementation.md, The chat composer is a textarea — the failed message comes back to the box instead of showing twice (the cleanup serves that rule and over-removes).
- **Suggested fix:** Use the same structural marker: push `{role:'user', content: what, pending: true}` and filter on `pending`, or capture `const at = session.length` before the push and splice that index on failure.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced the code and the described defect is real. In /Users/svein/dev/cookbook/web/src/routes/cookbook/+page.svelte, `draftRecipe` appends `{ role: 'user', content: what }` at line 41 and, on a throw from `chat`, cleans up at line 64 with `session = session.filter((t) => !(t.role === 'user' && t.content === what));` — a value-equality predicate over the whole session, not a removal of the one turn it pushed. The session array is genuinely multi-turn: assistant replies are appended at line 51 (`onDone`), the Composer placeholder at line 171 switches to 'Answer, or refine…' once `session.length > 0`, and the `#each` at line 149 renders all turns. So if the same short refinement text is sent twice ("spicier", "again", "more garlic") and the second attempt throws — `chat` does throw on a failed/non-ok response (web/src/lib/api.ts:49,58) and the `catch` at line 60 only early-returns when `ctl.signal.aborted`, which is not the case for a dropped connection or a server error — the filter removes both the earlier successful user turn and the failed one, orphaning the assistant reply between them. The contrast the finding draws with Thread.svelte is also accurate: Thread.svelte:92 tags the optimistic bubble with `created: ''` and line 115 filters on that structural marker (`m.created !== ''`), so it removes only unconfirmed bubbles. Severity 'low' is appropriate — the drafting transcript is intact server-side (linked at line 176, /page/threads/drafting), so this is display-only within the current session.

</details>

---

## E2E suite & evals

**Files:** `web/e2e/*.ts`, `web/e2e/serve.mjs`, `evals/src/main.rs`, `evals/fixtures/`  
**Read first:** implementation.md → *E2E*, *Evals*; CLAUDE.md → *No model in the test suite*  
**Key entry points:** the four Playwright specs, the overflow auto-fixture, the eval scenario runner  
**Theme:** The suite runs green on state a *previous* spec left in the shared corpus — three of the planning spec's assertions never exercise the exchange they claim to.

### 🟠 **MEDIUM** · #69 · The M4 planning spec's three load-bearing assertions are satisfied by messages and queue state composer.spec.ts left in the shared corpus, so the spec passes without the exchange ever happening

**`web/e2e/planning.spec.ts:25`** · _test-gap_

Playwright runs the four specs in one worker against one corpus in the order composer → cookbook → planning → recon. composer.spec.ts ends with a successful planning-thread exchange whose scripted fake answers `queue_add {title:'Dal', reason:'cheap'}` then `Queued dal.`, and both persist. Confirmed against the corpus a real run leaves in $TMPDIR: `export/threads/planning.md` contains two `> Queued dal.` messages and `export/queue.md` a single `dal | Dal | cheap` row created by composer.spec. So planning.spec's assertions at :25/:30/:31 all resolve against pre-existing state; only :27 (the echoed user text) still needs a successful round trip. It is also a latent hard flake: Thread renders every message, so once the new bubble paints the locator resolves to two elements and `toBeVisible` fails with a strict-mode violation.

- **Spec:** implementation.md → E2E: "Playwright drives the planning-session flow against the real server binary and a scripted fake Anthropic endpoint; deterministic"; CLAUDE.md → Testing: "Deterministic and reproducible".
- **Suggested fix:** Make each assertion name state only this test can have produced, or scope it to the new turn — assert on `page.locator('article.assistant').last()`, or give planning.spec a distinct prompt and reply text, and assert `why: cheap` inside the row this test added.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/e2e/planning.spec.ts:19-31; web/e2e/composer.spec.ts:23-27; web/playwright.config.ts:5-6; web/e2e/serve.mjs:103; web/src/lib/components/Thread.svelte:131-137


Failure scenario: Change serve.mjs:103 so the planning exchange performs no store mutation at all; `npx playwright test planning.spec.ts` still passes on composer.spec's leftovers. Conversely, on a loaded runner where the new reply renders before the first poll, the unmodified suite fails with `strict mode violation: getByText('Queued dal.') resolved to 2 elements`.

</details>

### ⚪ **LOW** · #65 · The composer contract's photo half — "a send that rejects puts the message **and any attached photos** back in the box" — has no test; both specs exercise a text-only send

**`web/e2e/composer.spec.ts:7`** · _test-gap_

The planning-thread case fills the textarea and presses Enter; the drafting case uses a composer with `photos` false. So the entire `files` restore path in `Composer.submit` (`const sent = files; … catch { files = sent; }`) is unexecuted. `recon.spec.ts` attaches files but never fails a `/chat` while photos are attached. The surrounding state is easy to get wrong: `sent` aliases the `$state` proxy array that `files = []` detaches, and `Thread.send`'s abort/`!live()` early-returns deliberately resolve — so widening one of those guards would silently eat the photos with the suite green.

- **Spec:** implementation.md, The chat composer is a textarea: "its contract includes failure: a send that rejects puts the message and any attached photos back in the box."
- **Suggested fix:** Add a case on /page/locations/home/pantry: `setInputFiles` twice, `route('**/chat', abort)`, send, then assert the error is visible, the textarea holds the message, the attach button still reads 📷2 and both filenames are still chipped. Then unroute and assert the retry succeeds with `[2 photos attached]` appearing exactly once — which also pins the no-duplicate-turn behaviour.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/e2e/composer.spec.ts:7-42; web/src/lib/components/Composer.svelte:41-49; web/src/lib/components/Thread.svelte:110,117; web/e2e/recon.spec.ts:22-33


Failure scenario: N/A (test gap). If `Thread.send` were changed to resolve instead of rethrow on any failure class, camera-captured photos would be destroyed and every existing e2e test would still pass.

</details>

### ⚪ **LOW** · #70 · The overflow auto-fixture filters culprits at `right > limit + 1` while asserting `scrollWidth - clientWidth === 0`, so sub-pixel overhangs fail the test with "(nothing found)"

**`web/e2e/helpers.ts:35`** · _test-gap_

`scrollWidth` is an integer derived from fractional layout, so an element overhanging by ~0.5-1.0 px pushes it to `limit + 1` and fails the assertion, while the diagnostic keeps only elements with `right > limit + 1` — which such an element is not. The prior review's #82 was recorded as done and genuinely improved (the new `scrollsInside` ancestor check correctly excludes elements inside an `overflow-x` container), but the threshold mismatch — the substance of the finding — was not changed. The secondary `[object SVGAnimatedString]` half is moot: there is no `<svg>` in the app markup.

- **Spec:** implementation.md → The phone is the tested shape: "an auto-fixture asserts no horizontal overflow at the end of every spec, naming the offending elements on failure".
- **Prior:** still-open
- **Suggested fix:** Filter on `right > limit` (or `limit + 0.5`) so the culprit set is a superset of what fails the assertion, and sort by `right` descending before `.slice(0, 8)` so the widest offender is always named.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (test-gap); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/e2e/helpers.ts:26-27,35,43-46


Failure scenario: A full-width element with a 1px border lands its right edge at 375.6px on a 375px viewport: scrollWidth becomes 376, the assertion fails, and because 375.6 is not > 376 the operator is told "past the edge: (nothing found)".

</details>

### ⚪ **LOW** · #72 · A scenario that returns `Err` aborts the run and exits 1, which is indistinguishable from "one mechanical check failed"

**`evals/src/main.rs:552`** · _quality_

The module header states the contract: "Exit code is the number of failed mechanical checks, so a shell loop can still notice regressions", and the unknown-scenario path reserves 255. But every scenario call in the dispatch loop uses `?`, so any error — a missing API key, a 400, a store error, a missing fixture — propagates out of main and exits 1. Remaining scenarios are skipped and the summary never prints, so a run that died in scenario 1 of 6 reads the same as a nearly-clean one. Same class as the (fixed) misspelled-scenario finding: a non-run must not read as a mostly-green run.

- **Spec:** evals/src/main.rs:1-7: "Exit code is the number of failed mechanical checks, so a shell loop can still notice regressions".
- **Suggested fix:** Catch the per-scenario error instead of `?`-ing it: print it, record it as a failed check (or a distinct counter), and continue; reserve a sentinel like 254 for "the run itself broke".

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: evals/src/main.rs:1-7,539-568


Failure scenario: Run with no `ANTHROPIC_API_KEY`: the first scenario errors, the process exits 1, and a wrapper loop records "1 regression" for a run in which zero checks were evaluated.

</details>

### ⚪ **LOW** · #73 · The `pantry-in-passing` check for "duck legs recorded as present" does a case-sensitive substring match on a model-supplied display name, so correct behaviour scores FAIL

**`evals/src/main.rs:229`** · _quality_

`pantry_set` takes `name` straight from the model and only falls back to the slug-derived lowercase form when it is absent, so `name: "Duck legs"` — the natural capitalization — makes `i.name.contains("duck")` false. Evals are read by a human as the prompt-regression signal, so a check that fails on correct behaviour trains the reader to ignore the line. The companion presence check is fine because presence is a closed vocabulary.

- **Spec:** implementation.md → Evals: "Scenarios seed a lived-in corpus and score mechanical checks".
- **Suggested fix:** Match on the slug key instead (`pantry.items.keys().any(|k| k.contains("duck"))`, since slugs are lowercased at ingress) or lowercase the name before comparing.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: evals/src/main.rs:227-230; crates/assistant/src/tools.rs:942-951


Failure scenario: The model calls `pantry_set {item:"duck-legs", name:"Duck legs", presence:"have"}` — exactly right — and the report prints `[FAIL] duck legs recorded as present`.

</details>

### ⚪ **LOW** · #74 · The `plan-week` anti-repetition check only inspects `dish.recipe`, so a duck curry queued as a free-text dish scores PASS

**`evals/src/main.rs:203`** · _quality_

`DishRefDoc` is `{recipe: Option<String>, title: String}` and `queue_add` accepts a title with no recipe link. The check asserts only that no dish has `recipe == Some("duck-curry")`, ignoring `title`. This is the one mechanical check standing in for the charter's priority-1 steering rule — the seed deliberately logs duck curry twice in the last week — and given `rotation::recency` is unreachable from the assistant, missing the unlinked case leaves the rule effectively unmeasured.

- **Spec:** design.md → Steering priority 1: track recency across cuisine, protein and format.
- **Suggested fix:** Also test the title: `d.recipe.as_deref() == Some("duck-curry") || d.title.to_lowercase().contains("duck curry")`.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: evals/src/main.rs:201-206; crates/store/src/pages.rs:95-98


Failure scenario: The model queues `{title: "Duck curry"}` with no recipe on a corpus whose log shows duck curry 2 and 5 days ago, and the report prints `[PASS] did not queue duck curry again`.

</details>

### ⚪ **LOW** · #75 · Every E2E run leaks its seeded corpus into $TMPDIR and never removes it; 50 have accumulated on this machine

**`web/e2e/serve.mjs:113`** · _quality_

`mkdtempSync(join(tmpdir(), 'mise-e2e-'))` creates the corpus root and the `process.on('exit')` handler only kills the server child. Each run leaves a SQLite store, a markdown export and a git repository with one commit per mutation. Disk litter today, but it also makes it easy to mistake a stale corpus for the current one while debugging, and macOS only sweeps $TMPDIR on reboot.

- **Suggested fix:** `fs.rmSync(root, {recursive: true, force: true})` in the exit handler alongside `server.kill()`, or print the path on exit so a failed run's corpus is deliberately kept and everything else cleaned.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: web/e2e/serve.mjs:113,142-143


Failure scenario: Four `npm run e2e` runs leave four more `$TMPDIR/mise-e2e-*` corpora that nothing ever reclaims.

</details>

---

## Packaging (Nix) & tooling

**Files:** `nix/module.nix`, `flake.nix`, `push.sh`, `pull.sh`, `Cargo.toml`  
**Read first:** implementation.md → *Server defaults* (module hardening), *Packaging* (M4)  
**Key entry points:** the hardened systemd unit, `ExecStartPre`, `ExecStart`  
**Theme:** The filesystem/kernel hardening is thorough, but the one control the SSRF deferral explicitly leans on — network-egress restriction — is absent.

### 🟠 **MEDIUM** · #76 · The unit the spec designates as the second line of defence against SSRF applies no network-egress restriction at all

**`nix/module.nix:139`** · _security_

`validate_url` checks the host textually (four literal suffixes plus IP literals), and both nix/module.nix:139-141 and implementation.md:317-320 say the systemd sandbox is the second line "sized for that job". The hardening block is entirely filesystem/kernel/privilege: NoNewPrivileges, ProtectSystem, ProtectHome, capability bounding, SystemCallFilter, RestrictNamespaces, PrivateDevices, ProtectProc, RemoveIPC. Nothing constrains where the process may connect — `RestrictAddressFamilies` deliberately permits AF_INET/AF_INET6 and there is no `IPAddressDeny=`/`IPAddressAllow=`, systemd's only knob acting on the post-DNS destination. A fetch resolving into the LAN, to the metadata address, or to loopback reaches its target exactly as with no sandbox. This is not the prior LOW finding (every directive it listed has since been added); the defect is that the added set omits the one control the SSRF deferral leans on.

- **Spec:** implementation.md, *`Fetch` is a seam like `Model`*: "Until then the systemd sandbox is the second line, and the module's hardening is sized for that job."
- **Suggested fix:** Add `IPAddressAllow = ["localhost"]` (needed for the Caddy loopback ingress) plus `IPAddressDeny` for 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10, fc00::/7 and fe80::/10; consider `MemoryDenyWriteExecute=true` and `SystemCallFilter=~@resources`. Then update implementation.md to state exactly what the second line does and does not cover (loopback-resolving hostnames remain reachable), or bring resolve-and-pin forward.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Traced all three pointers; every claim checks out.\n\n1. nix/module.nix:139-153 — the hardening block whose own comment reads \"This is the second line of defence for `fetch_url`, whose own guard is documented as not a bulletproof SSRF boundary\" consists solely of SystemCallFilter=@system-service, SystemCallArchitectures=native, RestrictAddressFamilies=[AF_INET AF_INET6 AF_UNIX], RestrictNamespaces, PrivateDevices, ProtectKernelModules, ProtectKernelLogs, ProtectClock, ProtectHostname, RestrictRealtime, ProtectProc=invisible, RemoveIPC — plus the earlier NoNewPrivileges/ProtectSystem/ProtectHome/CapabilityBoundingSet/ReadWritePaths group. All filesystem, kernel, and privilege controls. `grep -rn \"IPAddress\" /Users/svein/dev/cookbook/` returns zero hits: no IPAddressDeny=, no IPAddressAllow= anywhere in the repo. Those are systemd's only knobs that act on the post-DNS destination address. Line 144 affirmatively permits AF_INET and AF_INET6, so outbound sockets to any address are allowed.\n\n2. crates/assistant/src/fetch.rs:76-114 — validate_url matches parsed.host(); the Host::Domain arm (:83-93) lowercases and checks exactly four literals (localhost, .localhost, .local, .internal) then returns Ok(()). The private-range predicates reject_private_v4 (:116-130) and the v6 arm (:95-112, including the v4-mapped respelling fix) run only for url::Host::Ipv4/Ipv6 — i.e. only for IP literals. There is no resolution and no post-resolve check. HttpFetch::fetch (:178-196) passes the raw URL to reqwest, so the connect performs its own fresh resolution: validation is pre-connect and unpinned. redirect_ok (:136-141) delegates to the same validate_url, inheriting the identical blind spot.\n\n3. docs/implementation.md:313-320 states the textual/unresolved gap explicitly and then defers: \"Until then the systemd sandbox is the second line, and the module's hardening is sized for that job.\" The module's hardening is not sized for that job — it applies no network-egress restriction whatsoever.\n\nThe failure_scenario therefore holds exactly as described: a hostname with an A record of 192.168.1.1 or 169.254.169.254 clears validate_url's Domain arm, the sandbox permits the connection, and the response is extracted to markdown and returned as a tool result. Nothing in the code or the module refutes it. Distinct from the prior LOW finding — every directive that one named is now present; the defect here is the omission of the single control the documented deferral relies on. Medium severity is fair: a real SSRF read primitive reachable via injected content in a fetched page, bounded by this being a single-user personal server.

</details>

### ⚪ **LOW** · #77 · ExecStartPre chowns only the leaf of `services.mise.root`, so any intermediate directory it creates stays root-owned at 0700 under UMask=0077 and the service user cannot traverse into its own corpus

**`nix/module.nix:114`** · _bug_

The privileged ExecStartPre exists so a root outside /var/lib/mise works instead of hitting a read-only filesystem. It runs `mkdir -p $root` then a non-recursive `chown mise:mise $root`; `UMask = "0077"` applies to every command of the unit including `+`-prefixed ones (the prefix drops privilege/sandbox application, not the umask), so any parent `mkdir -p` creates is 0700 root:root and never chowned. The `chmod -R go-rwx` only walks downward. The `mise` user then has no search permission on the parent — the same confusing restart loop the ReadWritePaths fix was meant to eliminate, with EACCES instead of EROFS. The default root and roots whose parent already exists are unaffected.

- **Spec:** implementation.md, Server defaults: "that same `ExecStartPre` runs privileged and creates the root wherever it points … so a root outside `/var/lib/mise` serves instead of hitting a read-only filesystem."
- **Suggested fix:** Create parents with a traversable mode explicitly (`install -d -m 0755 "$(dirname $root)"` then `install -d -m 0700 -o mise -g mise $root`), or assert in the module that `cfg.root`'s parent must already exist; stop relying on `mkdir -p` inheriting a usable mode from UMask.

<details><summary>Verification trail — code pointers</summary>

**Verdict: confirmed.** Verified in nix/module.nix. ExecStartPre (line 114-118) runs `mkdir -p $root`, a non-recursive `chown mise:mise $root`, and `chmod -R go-rwx $root` — the chmod only descends, so an intermediate directory created by `mkdir -p` is never touched. UMask=0077 (line 105) is an exec-context property that the `+` prefix does not disable (the prefix suppresses User=/Group=, capability, namespace and sandbox settings, not UMask=), so such an intermediate directory is created 0700 root:root. The service body runs as User=mise/Group=mise (lines 97-98), and ReadWritePaths=[cfg.root] (line 132) bind-mounts the leaf writable but grants no DAC search bit on the parent. crates/store/src/store.rs:211-215 has Store::open stat `root.join("mise.db")` first thing, which fails under EACCES and yields NoCorpus (or create_bare fails outright with --init); Restart=on-failure with RestartSec=5 (lines 119-120) then produces the described restart loop. The stated scenario (root=/data/mise/cookbook with /data present, /data/mise absent) is reachable. Severity low is correct: the default root is pre-created by StateDirectory=mise (line 99) and any root whose parent already exists is unaffected, so only a non-default root with a missing intermediate directory hits it.

</details>

### ⚪ **LOW** · #78 · `ExecStart` interpolates the free-form root, listen, model and web-app options into a systemd exec line without escaping, while the sibling `ExecStartPre` correctly escapes the same path

**`nix/module.nix:92`** · _quality_

ExecStart is built by string concatenation from unvalidated `types.str` options; systemd word-splits exec lines on whitespace and expands `%` specifiers, so a value with a space becomes two arguments and a `%` either expands unintended or fails unit load. The asymmetry is the tell: `ExecStartPre` at :114-118 already uses `lib.escapeShellArg` for the same values. The failure mode is maximally unhelpful — the privileged pre-step succeeds and creates the directory, so the operator sees a correctly-provisioned corpus root next to a service that will not start. The same concatenation is what the planned `instances` attrset will template over.

- **Spec:** implementation.md, Server defaults — the ExecStartPre creates the root wherever `services.mise.root` points.
- **Suggested fix:** Build the command with `lib.escapeSystemdExecArgs [ (lib.getExe cfg.package) "--root" cfg.root "--listen" cfg.listen … ]`, which quotes each argument and escapes `%`.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: nix/module.nix:24-28,92-96,114-118


Failure scenario: `services.mise.root = "/srv/my cookbook"`: ExecStartPre creates the directory correctly, ExecStart passes `--root /srv/my` plus a stray `cookbook`, clap rejects the positional, and `Restart=on-failure` turns it into a 5-second crash loop against a directory that looks correctly provisioned.

</details>

### ⚪ **LOW** · #79 · The packaged Rust derivation ships the eval harness binary, so `nix profile install` puts `mise-evals` on end users' PATH and the module's store path carries it into deployments

**`flake.nix:14`** · _quality_

`buildRustPackage` installs every workspace binary and the workspace includes `evals`. Verified: the built `bin` directory contains `mise`, `mise-server` and `mise-evals`. The eval runner is documented as a hand-run harness against the real Anthropic API, kept out of CI and out of pass/fail gates; it is not part of the product surface, seeds throwaway corpora, and spends money when run. `postInstall`'s `wrapProgram` loop also wraps it, giving it git on PATH as if it were a supported entry point.

- **Spec:** implementation.md, Evals: "`evals/` is a workspace binary … but only runs by hand against the real API"; CLAUDE.md: evals are "never as pass/fail gates in CI."
- **Suggested fix:** Restrict the build to shipped crates (`cargoBuildFlags = ["-p" "mise-cli" "-p" "mise-server"]`, keeping workspace-wide `cargoTestFlags` so evals still compiles under check), or delete `$out/bin/mise-evals` in postInstall before the wrap loop.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (quality); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: flake.nix:14-29; Cargo.toml:3


Failure scenario: A user who tab-completes `mise<TAB>` after `nix profile install` runs `mise-evals` and starts a scenario run against the real Anthropic API from a binary never meant to be part of the installed product.

</details>

---

## Docs

**Files:** `docs/implementation.md`  
**Read first:** the doc/code agreement invariant in CLAUDE.md  
**Key entry points:** the on-disk layout diagram, the uid-decision record  
**Theme:** Two small omissions where the doc under-describes what the code emits.

### ⚪ **LOW** · #18 · The on-disk layout diagram omits `shopping.md`, a global page the render layer has always emitted

**`docs/implementation.md:543`** · _spec-drift_

`render()` unconditionally writes `shopping.md` at the export root, the shopping list is listed as a structured page at implementation.md:514, and it has UI edit actions — but the authoritative layout tree at :537-554 does not mention it. That tree is what an auditor uses to check export completeness, so the code is right and the doc is incomplete.

- **Spec:** CLAUDE.md, Documents — "When you find the two disagreeing, that's a bug in one of them."
- **Suggested fix:** Add `shopping.md    # the tiered shopping list` next to queue.md/someday.md and bump the doc's last-updated tag.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: docs/implementation.md:514,543-553; crates/store/src/render.rs:89; crates/server/src/api.rs:225-227


Failure scenario: n/a — documentation gap; a completeness audit against the layout tree would wrongly conclude shopping state has no export home.

</details>

### ⚪ **LOW** · #83 · The implementation doc records the replica-scoped log/thread uid decision in detail but never mentions the collection-item id scheme, an equally permanent stored-data format decision

**`docs/implementation.md:460`** · _spec-drift_

implementation.md:84-98 and :459-469 document `sha256(entry)[..16]-<replica-id>-<n>` at length. Nothing in implementation.md or design.md mentions the `<prefix>-<replica>-<seq>` shape for shopping items and fridge portions, the `meta.id_seq` counter, or the rule that legacy positional `s1`/`p1` keys stay inert; grepping for `id_seq`, `mint`, `positional`, `map key` finds nothing. The only record is a code comment and a closed remediation checkbox. This is exactly the class of decision the doc says it records, and the gap is what let the CLI keep its own allocator — there was no doc rule for the CLI change to violate.

- **Spec:** CLAUDE.md → Documents: "When a change alters behavior the docs describe, the doc edit belongs in the same change."
- **Suggested fix:** Add a bullet next to Log-row identity: collection-item ids are `<prefix>-<replica-id>-<seq>` from a monotonic per-store `meta.id_seq`; they are CRDT map keys, so every write path must mint through `Store::mint_id` and none may derive an id from content or from a local-map scan; pre-existing positional keys remain valid and are never reused or renumbered. Bump the last-updated tag.

<details><summary>Verification trail — code pointers</summary>

_Non-bug finding (spec-drift); not subject to the disprove pass — trail is the finder's own reasoning._


Pointers: docs/implementation.md:84-98,459-469; crates/store/src/store.rs:479-496; docs/remediation.md:63


Failure scenario: N/A (documentation gap). Concretely: a contributor adding a surface that inserts into ShoppingDoc/FridgeDoc finds a detailed rule for log uids and none for item ids, and writes a local-scan allocator — which is what happened to `mise fridge add`.

</details>

---
