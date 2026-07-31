# Mise Codebase Review — Remediation Report

_Generated 2026-07-31 by a multi-agent audit (15 Opus finder agents, one per
area; every bug/security finding then independently adversarially verified).
117 raw findings → 94 kept (61 confirmed, 2 uncertain, 31 non-bug findings that
skip the disprove pass), 1 refuted._

_Revision: jj change `yyxrwrxknyum` · commit `afa7cdefb408`. Scope: whole codebase._

<!-- audit-revision
mode: whole
commit: afa7cdefb408
jj-change: yyxrwrxknyum
generated: 2026-07-31
-->

Per `CLAUDE.md`, fixes want a failing regression test **first** — except where
the fix makes the bug class unrepresentable in the types, in which case say so
and skip the test rather than bending the design to keep the bug reachable.
Several findings below are exactly that second kind and are marked.

## Executive summary

The codebase is in good shape for its age: the layering the docs promise is
real (`core` depends on nothing, `store` on `core` only), the seams (`Model`,
`Fetch`) hold, the clock genuinely is a parameter below the edges, and the
property tests exist where the charter says they should. Nothing here suggests
a rewrite; the findings are the ordinary consequence of a system that grew
fast, plus three or four places where a promise in the docs is stated more
strongly than the code delivers.

The findings that actually matter cluster in four places:

**Stored XSS reaching the bearer token.** `Markdown.svelte` renders page
content with `{@html}` through `marked` with no sanitizer, defended only by a
comment asserting the content is "our own render output, not third-party
input". `fetch_url` makes that premise false: a hostile recipe site's JSON-LD
flows through extraction into a recipe body, into the export byte-for-byte
(`esc()` never escapes `<`), and back out to the browser, where the payload can
read `localStorage['mise-token']` — the single static credential for read,
write, revert and chat over the whole corpus.

**Silent data loss on the sync path.** Two independent mechanisms. `Peer` holds
each Automerge doc in memory for a whole session and writes that stale copy as
the periodic snapshot, so any change another writer commits mid-session is
erased once the snapshot boundary is crossed. Separately, `shopping_add` and
`fridge_add` allocate ids by scanning for the lowest free `s<n>`/`p<n>` in the
*local* replica, so two devices that add different items while apart both pick
`s1` and one item is destroyed on merge — which is precisely the offline
shopping-list scenario the charter names as the motivating case.

**The export can stop telling the truth, permanently.** `git init` runs exactly
once, at corpus creation; delete `export/` (which the docs explicitly call
"deletable and regenerable at any time") and every subsequent mutation fails
*after* the store has already been written. And a sync session that carries
only thread messages skips the export entirely, so synced transcripts live in
SQLite and nowhere else — the one state the charter forbids.

**Recurring root causes.** Three themes explain most of the list. (1) *Two
ingress paths, one validated*: the local append paths normalize, validate and
hash content, while the sync insert paths take peer bytes verbatim — no uid
verification, no normalization, no round-level transaction. (2) *Hand-copied
logic drifting*: the CLI reimplements `views::render_queue_status` rather than
calling it; `Store::revert` hand-enumerates recipe fields and has already lost
`source`; the cookbook composer is an `<input>` while the spec and the thread
composer say textarea. (3) *Error paths abandoning state*: a failed exchange
leaves store mutations unexported and the thread dangling, a failed send
destroys the user's draft and camera photos, `applyAll` erases its own error
banner.

### Fix-first order

1. 🔴 **Sanitize rendered markdown** — `web/src/lib/components/Markdown.svelte:41` (`{@html}` sink). Configure `marked` to escape raw HTML (nothing in the corpus needs inline HTML) or bundle DOMPurify; add a CSP. One-file fix, closes the token-theft path.
2. 🔴 **Stop writing stale sync snapshots** — `crates/store/src/sync.rs:183` (`Peer::commit` → `Store::append_changes`). Rebuild the doc inside the snapshot transaction instead of saving the caller's copy. Silent, permanent data loss today.
3. 🔴 **Replica-safe ids for shopping items and fridge portions** — `crates/assistant/src/tools.rs:1131`, `:1002`. Content-hash + occurrence (as the log already does) or a device-prefixed id; extend the convergence property to allocate through the real tool path.
4. 🔴 **Make `export()` self-healing** — `crates/store/src/store.rs:830`. `git init` when `.git` is absent before the status/add/commit sequence. Restores the documented "deletable and regenerable" property.
5. 🔴 **Close the IPv4-mapped IPv6 SSRF bypass** — `crates/assistant/src/fetch.rs:67`. Map v4-mapped/v4-compatible forms back through the IPv4 predicate. Two-line fix; the test table already exists.
6. 🔴 **Export after a thread-only sync** — `crates/cli/src/main.rs:400`. Drop the guard (export is a no-op when nothing changed) or match the server's condition.
7. 🔴 **Restore `source` on recipe revert** — `crates/store/src/store.rs:605`. Destructure the hydrated doc so the next added field is a compile error rather than a silent omission.
8. 🔴 **Stop destroying the composer's draft and photos on a failed send** — `web/src/lib/components/Thread.svelte:55`. Clear only after `chat()` resolves; restore on failure. The motivating environment is a shop basement with bad signal.
9. 🟠 **Move auth into a tower layer** — `crates/server/src/lib.rs:123`. Removes the pre-auth body buffering, makes new routes authenticated by default, and lets the ?token= fallback be scoped to the WebSocket that actually needs it (`:156`).
10. 🟠 **Tighten the corpus file modes** — `nix/module.nix:94`. `UMask=0077` + `StateDirectoryMode=0700`; today transcripts, photos and the cook log are world-readable on the host.

## Region index

| Region | 🔴 | 🟠 | ⚪ | Total |
|---|---:|---:|---:|---:|
| Domain core |  | 3 | 3 | 6 |
| Store — persistence, history, revert | 1 | 3 | 1 | 5 |
| Store — markdown export | 1 | 2 | 1 | 4 |
| Store — sync & threads | 1 | 4 | 3 | 8 |
| Assistant — Anthropic client |  | 3 | 3 | 6 |
| Assistant — turn, exchange, context |  | 3 | 3 | 6 |
| Assistant — tools & views | 1 | 4 | 5 | 10 |
| Assistant — fetch & recon | 1 | 4 | 6 | 11 |
| Server | | 5 | | 5 |
| CLI & remote | 1 | 5 | 4 | 10 |
| Web client | 2 | 6 | 3 | 11 |
| E2E & repo tooling |  | 2 | 3 | 5 |
| Packaging (Nix) & evals |  | 2 | 4 | 6 |
| Docs |  |  | 1 | 1 |
| **Total** | **8** | **46** | **40** | **94** |

Severity legend: 🔴 **HIGH** (fix first), 🟠 **MEDIUM**, ⚪ **LOW** (spec-drift,
dead code, doc precision, test gaps).

---

## Domain core

**Files:** `crates/core/src/{readiness,coverage,rotation,types}.rs` (pure domain
math), `crates/core/tests/properties.rs` (the charter's invariant properties)
**Read first:** design doc → *The Queue* (readiness, lead time) and *Steering*;
`CLAUDE.md` → *Domain invariants are properties*
**Key entry points:** `readiness::assess`, `coverage::coverage`,
`rotation::recency`
**Theme:** the math is right in the regime the generators explore; the defects
sit at the domain edges the generators refuse to visit, and one whole module is
never called from production.

### 🟠 MEDIUM · `coverage()` panics instead of saturating on large serving counts

**`crates/core/src/coverage.rs:43`** · _bug_

`runs_out`/`runs_out_with_freezer` build a `jiff::Span` with
`i64::from(dinners).days()`. jiff panics when `|days| > 7_304_484`, and the
panic happens *during span construction*, before `Date::saturating_add` gets a
chance to saturate. The `u32::try_from(..).unwrap_or(u32::MAX)` in `dinners()`
is a phantom guard: `u32::MAX` is roughly 588× past the limit, so the
saturating branch guarantees the panic rather than preventing it. Nothing
bounds servings on the write path (`PortionDoc.servings` is a bare `u32`,
`fridge_add` validates only that recipe servings are non-zero), so one absurd
number is persisted, syncs to every device, and every later read of the queue
(`/api/queue`, the `queue_status` tool, `mise queue`) panics.

- **Spec:** design doc, *Graceful decay*: "slightly stale suggestions, not a broken database demanding reconciliation."
- **Suggested fix:** clamp the horizon before building the span
  (`min(7_304_484).days()`), or use `Span::new().try_days()` falling back to
  `Date::MAX`; drop the misleading `unwrap_or(u32::MAX)`. Independently bound
  servings at the tool/CLI ingress.

<details><summary>Verification trail — code pointers</summary>

Confirmed by reproduction. A scratch test with a single 100,000,000-serving
fridge portion at headcount 2, run with `cargo test -p mise-core`, panics:
`jiff-0.2.35/src/span.rs:788: value for days is out of bounds: parameter 'days'
is not in the required range of -7304484..=7304484`. Write path unguarded at
`crates/assistant/src/tools.rs:977,1009` and `crates/store/src/pages.rs:229`;
unconditional read-path calls at `crates/assistant/src/views.rs:114` and
`crates/cli/src/main.rs:894`. One correction to the original finding: the
`all_dinners - fridge_dinners` subtraction at `coverage.rs:44` cannot underflow,
since both totals saturate to the same `u32::MAX`. Severity was lowered from
high to medium on verification: the trigger needs ~14.6M total servings, so it
is reachable only via an absurd model or user entry, not any normal flow.

</details>

### 🟠 MEDIUM · The coverage property never generates the states that panic

**`crates/core/tests/properties.rs:138`** · _test-gap_

`arb_portion` draws servings from `0..=12` and the property draws at most 8
fridge plus 8 freezer portions, capping totals at 192 dinners — six orders of
magnitude below the jiff day limit. The `u32::MAX` saturation branch in
`dinners()` has zero coverage. The oracle also uses
`checked_add(...).unwrap()`, which would panic in the same place, so widening
the generator alone is not enough: the oracle has to move to the same clamped
formula as the fixed `coverage()`.

- **Spec:** `CLAUDE.md`: "Coverage is monotone in fridge servings" as a property over generated states.
- **Suggested fix:** widen the servings range to cross the clamp, and rewrite
  the `runs_out` oracle to use the clamped formula. This is the failing test
  that should land before the `coverage()` fix above.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding; trail is the finder's own reasoning. `properties.rs:137-143`
(generator), `:238-250` (oracle), `coverage.rs:25-27` (the uncovered branch).
The suite is green only because the generator refuses states the write path
accepts.

</details>

### 🟠 MEDIUM · The anti-curry engine's rotation math is unreachable from the assistant

**`crates/core/src/rotation.rs:29`** · _architecture_

`rotation::recency` is the computational basis of steering priority 1, and its
only production caller is a CLI debug subcommand. The assistant's tool list has
no rotation tool, and `context::assemble` injects state, steering and facts
plus the thread's page — never recency, never the log. Grepping `recency` across
`crates/assistant`, `crates/server` and `crates/store` returns nothing. The
system prompt nevertheless instructs the model to check the log for recency and
counterweight repeated axes, so in production the model must eyeball raw log
markdown and do date arithmetic in its head — exactly the judgment the charter
places below the seam.

- **Spec:** design doc, *Steering* priority 1: "Track recency across cuisine, protein, and format…"
- **Suggested fix:** add a rotation read tool returning `recency(&log,
  ctx.today(), window)`, and/or fold a compact recency summary into the planning
  thread's context block. Either way the model stops doing date arithmetic.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `rotation.rs:29` (the function), `tools.rs:189-451` (the tool
list, no rotation entry), `context.rs:30,60,96-114` (what actually gets
injected), `cli/src/main.rs:837` (the debug subcommand that is its only
caller).

</details>

### ⚪ LOW · Duplicate shopping needs when a recipe references one pantry item twice

**`crates/core/src/readiness.rs:73`** · _bug_

`assess()` pushes a `ShopNeed` for every ingredient *line* whose linked pantry
item is Out or absent. `Readiness.shop` has no de-duplication and both renderers
map straight over it, so a recipe that legitimately names one pantry item on
several lines (soy sauce in the marinade and again in the sauce) shows the item
twice in the queue's shop verdict. `recipe_add`/`recipe_edit` do not dedupe
pantry links either. Rank and tier are unaffected — it is a duplicated
user-facing list.

- **Spec:** design doc, *The Queue*: "Readiness: … does it need a shop trip — and which tier".
- **Suggested fix:** collect needs into a `BTreeMap<Slug, Option<Slug>>` so a
  `ShopNeed` is one per pantry item, producing the ordered `Vec` from the map.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `readiness.rs:73` (the push, inside the per-ingredient loop); both
renderers consume `Readiness.shop` without dedupe.

</details>

### ⚪ LOW · Defrost readiness is promised by the design and absent from the code

**`crates/core/src/readiness.rs:59`** · _spec-drift_

`assess()` takes the whole `LocationView` but reads only equipment and pantry —
never `location.freezer`. Nor could it: `Portion` is `{dish, servings, date}`,
cooked batches only, with no pantry link and no raw/cooked distinction, and
`PantryItem` has have/low/out with no frozen state. No milestone schedules the
work either, so the docs and the code disagree and one of them is wrong.

- **Spec:** design doc, *The Queue*: "frozen raw proteins add a defrost step to readiness."
- **Suggested fix:** settle it with the user rather than guessing — either drop
  the sentence from the design doc, or schedule the work and give the model a
  representation (a frozen presence variant, or freezer entries carrying a
  pantry slug plus a defrost lead) so `assess()` can emit `AfterLead`.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `readiness.rs:59` (reads equipment and pantry only);
`core/src/types.rs` (`Portion`, `PantryItem` shapes). Per `CLAUDE.md`'s
documents rule, this is a disagreement to resolve with the user before either
side is edited.

</details>

### ⚪ LOW · The lead-time property restates the implementation

**`crates/core/tests/properties.rs:221`** · _test-gap_

The "consistent under time shift" property mirrors the implementation's own
formula rather than the specification's statement of it, and its generator
excludes the only regime where `act_by` and `ready_at` are not inverses. A
property that recomputes the implementation cannot fail when the implementation
is wrong.

- **Spec:** `CLAUDE.md`: "ready at t with lead L ⟺ the act-now step is due at t−L", tested as a property.
- **Suggested fix:** state the property in the spec's terms (assert the
  biconditional directly over generated states) and widen the generator to
  include the excluded regime.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `properties.rs:221` and the surrounding generator.

</details>

---

## Store — persistence, history, revert

**Files:** `crates/store/src/store.rs` (SQLite, Automerge, history, revert),
`pages.rs` (doc types), `docid.rs` (the export-path authority), `error.rs`
**Read first:** implementation doc → *The page model*, *Revert semantics*,
*First-cook promotion lives in `Store::append_log`*
**Key entry points:** `Store::create_doc`, `append_changes`, `revert`,
`corpus`, `export`
**Theme:** the CRDT layer is sound; the failures are at the SQL boundary — two
transactions where one is needed — and in one hand-enumerated field list that
has already drifted.

### 🔴 HIGH · `Store::revert` on a recipe silently keeps the current `source`

**`crates/store/src/store.rs:605`** · _bug_

The `DocId::Recipe` arm hand-enumerates the fields it restores —
`schema_version`, `title`, `servings`, `effort`, `lead`, `tags`, `equipment`,
`ingredients`, `status`, plus the body splice — and never touches `source`.
`source` is a live mutable field: `recipe_edit` sets and clears it, it is part
of `RecipeDoc`'s `PartialEq`, and it renders into the export frontmatter. So
reverting a recipe to a point before it had a source leaves the source in
place, and a wrong source URL cannot be undone from the history UI at all.
Structured pages use `revert_plain` (whole-value assign) and are safe; the
`Technique` arm happens to be complete today. The hand-written list is the real
defect: every future `RecipeDoc` field silently opts out of revert.

- **Spec:** implementation doc, *Revert semantics*: "Property: revert reaches every point in history exactly."
- **Suggested fix:** destructure the hydrated `RecipeDoc` in the closure
  (`RecipeDoc { schema_version, title, …, body: _ }`) so adding a field is a
  compile error rather than a silent omission — this makes the bug class
  unrepresentable, so per `CLAUDE.md` the regression test is optional. Apply the
  same to the `Technique` arm, and extend the revert property to recipes anyway
  (see the next finding).

<details><summary>Verification trail — code pointers</summary>

Confirmed by tracing. `RecipeDoc.source: Option<String>` at
`crates/store/src/pages.rs:296`, included in the hand-written `PartialEq` at
`:313`, so it is observable state. Revert arm at `store.rs:603-621` assigns
exactly nine fields and calls `update_body`; `source` appears nowhere.
Mutability confirmed at `crates/assistant/src/tools.rs:824-827` (`recipe_edit`)
and `:732` (`recipe_create`). Export rendering at
`crates/store/src/render.rs:321-323`.

</details>

### 🟠 MEDIUM · `create_doc` splits the doc row and its first change across two transactions

**`crates/store/src/store.rs:334`** · _bug_

The `INSERT INTO docs` runs on the bare auto-commit connection; `persist_change`
then opens its own transaction. A failure between them — `SQLITE_BUSY` from a
second process (no `busy_timeout` is set, see below), disk full, a kill — leaves
a `docs` row with zero change rows. `load_doc` then returns an *empty*
`AutoCommit` rather than `NotFound`, `get::<RecipeDoc>` fails with a hydrate
error, and `corpus()` fails wholesale — which takes down every export,
`/api/pages`, `/api/page` and `list_pages`. `create_doc` for the same id then
returns `Exists`, so the app cannot repair itself. `Peer::commit` has the same
split (`ensure_doc_row` then `append_changes`), though there a later sync heals
it.

- **Spec:** implementation doc, *The page model*: "The store is the truth."
- **Suggested fix:** wrap the docs insert and the first change row in one
  transaction (an inner variant of `append_changes` taking `&Transaction`); do
  the same for `ensure_doc_row` + `append_changes` in `Peer::commit`. Regression
  test: inject a failure between the two writes and assert the corpus still
  reads.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `create_doc` at `store.rs:320-342` — `exists` check, then a bare
`self.conn.execute` INSERT at `:334`, then `persist_change` → `append_changes`,
which opens its own transaction at `:263` and commits at `:295`. Consequence
chain verified: `load_doc` at `:184-218` returns `Ok(empty)` when there are no
snapshots and no changes; `get::<T>` hydrates from that empty doc at `:239-241`
and fails.

</details>

### 🟠 MEDIUM · `corpus()` enumerates locations from pantry docs alone, then requires all four siblings

**`crates/store/src/store.rs:767`** · _bug_

The location list comes from `list("pantry")` and the other three kinds are
fetched unconditionally. Two bad outcomes. Pantry row absent but siblings
present: the location vanishes from `CorpusState` with no error, and `export()`
then deletes `locations/<name>/*.md` as stale — so store state is legible
nowhere in the export. Pantry present but a sibling absent: `corpus()` errors,
so `export()` fails at its first line and *no* page is regenerated, which the
server swallows as a warning. Partial doc sets are reachable because doc rows
are created one auto-commit at a time and `Peer::commit` skips docs with no
changes.

- **Spec:** implementation doc, *Risks*: "anything the export omits silently breaks the exit-strategy promise."
- **Suggested fix:** enumerate locations from the union of the four kinds (or
  from `StateDoc.locations`) and treat a missing sibling as empty, or surface a
  `Corrupt` error naming the gap. Add a test pinning whichever behaviour is
  chosen.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `corpus()` at `store.rs:765-776` iterates `self.list("pantry")` then
calls `get::<EquipmentDoc>/<ShopsDoc>/<FridgeDoc>` for that slug; `list(kind)`
is a plain `SELECT id FROM docs WHERE kind = ?1` at `:229-237`. A missing
sibling is a hard error via `load_doc`'s `NotFound` at `:184-192`, and
`export()` calls `render(&self.corpus()?)` as its first statement at `:831`.
Stale-file deletion at `:842-848`.

</details>

### 🟠 MEDIUM · The revert property covers only pantry docs

**`crates/store/tests/store_behavior.rs:353`** · _test-gap_

`revert_reaches_every_point_in_history` generates operation sequences against
`DocId::Pantry` only — which reverts via `revert_plain`, a whole-value assign
that is correct by construction. The paths that *can* drift, Recipe and
Technique with their hand-enumerated field lists, are covered only by
`revert_restores_prose_pages_including_non_ascii_bodies`, which asserts title,
servings and body. `source`, `lead`, `tags`, `equipment`, `ingredients` and
`status` are never checked. That is exactly why the missing `source` restore
above is live and green.

- **Spec:** implementation doc M4: "Property: revert reaches every point in history exactly"; `CLAUDE.md`: high-risk areas get extra coverage.
- **Suggested fix:** parameterize the property over doc kinds — drive a
  `RecipeDoc` through generated edits touching every field, snapshot after each,
  revert to the k-th change and assert full-struct equality. Same for a
  technique.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `store_behavior.rs:322-343` (the pantry-only property),
`:350-385` (the three-field prose example), `store.rs:603-631` (the two
hand-enumerated arms).

</details>

### ⚪ LOW · No `busy_timeout` or WAL mode on the SQLite connection

**`crates/store/src/store.rs:169`** · _bug_

`Connection::open` is used bare in both `create_bare` and `open`; there is no
`PRAGMA busy_timeout`, `journal_mode` or `foreign_keys` anywhere. SQLite's
default busy timeout is 0, and the rollback journal takes an exclusive lock for
the write commit while `append_changes` uses a deferred transaction that reads
`MAX(seq)` before writing, so it needs a lock upgrade. Any overlap between the
running server and a local `mise` command — which the design supports and the
dev `.env` encourages — surfaces as an immediate "database is locked",
sometimes mid multi-statement operation.

- **Suggested fix:** run `PRAGMA journal_mode=WAL; busy_timeout=5000;
  foreign_keys=ON` in both `create_bare` and `open`, and consider an immediate
  transaction for `append_changes`.

<details><summary>Verification trail — code pointers</summary>

Confirmed. No `PRAGMA` anywhere in `store.rs`; deferred transaction at `:263`
with the `MAX(seq)` read preceding the first write.

</details>

---

## Store — markdown export

**Files:** `crates/store/src/render.rs` (deterministic rendering),
`crates/store/tests/export.rs` (determinism + completeness properties),
`crates/store/tests/support/mod.rs` (the test-only parser and generators)
**Read first:** `CLAUDE.md` → *The export never lies*; implementation doc →
*On-disk layout*, *Risks: export drift*
**Key entry points:** `Store::export`, `render`, `esc`, `support::parse_corpus`
**Theme:** rendering is deterministic as promised, but the escape alphabet is
incomplete and the property's generators only ever produce already-normalized
values — so the round-trip is verified on a strict subset of representable
states.

### 🔴 HIGH · The export is not regenerable: deleting it breaks every mutation, permanently

**`crates/store/src/store.rs:830`** · _bug_

`git init` runs exactly once, in `create_bare`. `Store::open` never checks the
repo, and `export()` goes straight to `git status --porcelain`. It *does*
re-create the directories as a side effect of `create_dir_all(parent)` and
rewrites every file — but it never re-inits the repo, so the git step fails with
"fatal: not a git repository" forever after. The blast radius is the whole app:
every CLI mutation ends in `store.export(...)?` and the HTTP edit path returns
`fail(e)` — *after* the SQLite mutation has already committed. Users naturally
retry, and `append_log`'s `<hash>-<occurrence>` uid then inserts a genuine
duplicate row, so one deleted directory becomes silent log corruption.

- **Spec:** implementation doc, *On-disk layout*: "The export is derived — deletable and regenerable at any time — and kept as a git repo."
- **Suggested fix:** make `export()` self-healing — `create_dir_all` the export
  dir and `git init -q` when `.git` is absent, before the status/add/commit
  sequence. Regression test first: create → export → `remove_dir_all` → reopen →
  export succeeds with `rev-list` count 1.

<details><summary>Verification trail — code pointers</summary>

Confirmed with a throwaway integration test covering both a full delete and a
`.git`-only delete. `git init` appears exactly once, at `store.rs:145` inside
`create_bare`; the only other git invocations are `status --porcelain`, `add
-A`, `commit` at `:851-854`. `Store::open` at `:164-172` checks only that
`mise.db` exists. Callers that fail after committing: `crates/cli/src/main.rs:831`,
`crates/server/src/api.rs:306`.

</details>

### 🟠 MEDIUM · `status` is the one frontmatter string rendered without escaping

**`crates/store/src/render.rs:324`** · _bug_

Every other string in `recipe_page` goes through `esc` — title, effort, lead,
tags, equipment, source — but line 324 pushes `r.status.to_string()` raw, and
the test-only parser mirrors the omission. Demonstrated: a status of
`"act\nsource: evil"` renders a forged `source: evil` frontmatter line and
round-trips status back as `"act"`; a status of `"x\n---\ntitle: other"` would
terminate the frontmatter block early. This is latent today only because
`parse_status` narrows the domain — and the property strategy only ever emits
`draft|active|retired`, so the test can never see it. Nothing in the types stops
a future write path or a differently-versioned peer from widening the field.

- **Spec:** `render.rs:6-8` — values that could break line structure are escaped, and the parser can reverse it exactly.
- **Suggested fix:** `esc(&r.status)` plus `unesc` in the parser. Better: give
  status (and effort, presence) a real enum with `Reconcile`/`Hydrate` impls so
  out-of-vocabulary values are unrepresentable — which is the
  correct-by-construction version and removes the need for the test.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `recipe_page` at `render.rs:304-324`: `esc` on title (`:307`), effort
(`:309`), lead-step (`:313`), `tag_esc` on tags (`:316`), `esc` on each
equipment element (`:319`) and source (`:322`); line 324 is the sole exception.
`frontmatter` at `:63-69` does a bare `writeln!(out, "{k}: {v}")` with no
escaping of its own. Parser mirror at `tests/support/mod.rs:321,623`.

</details>

### 🟠 MEDIUM · The export property only generates already-normalized corpora

**`crates/store/tests/support/mod.rs:451`** · _test-gap_

Normalization — trimmed strings, `None` rather than empty, LF line endings —
lives only as convention in `tools.rs`'s `must_trim`/`opt_trim`, while the doc
types are plain `String`/`Option<String>`. The generators bake the same
assumption in (`text()` trims, `opt_text()` filters empties), so the "export
never lies" property is verified on a strict subset of representable states. The
render/parse pair is genuinely lossy outside that subset: a log title
`"  padded  "` → `"padded"`, a shopping tier `Some("")` → `None`, thread content
`"a\r\nb"` → `"a\nb"`. And `Peer::handle` inserts remote rows verbatim via
`insert_log_row`/`insert_thread_row`, which apply none of `append_*`'s
normalization — so the unnormalized states are not hypothetical, they arrive
over sync.

- **Spec:** `CLAUDE.md`, *The export never lies*: completeness verified by export → parse → structural compare.
- **Suggested fix:** normalize inside `insert_log_row`/`insert_thread_row` so
  every path lands normalized (this also fixes finding *sync insert bypasses
  normalization* below), then widen `arb_corpus` to generate raw strings while
  asserting the normalized corpus round-trips.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `support/mod.rs:117` (`text()`), `:451-467` (`arb_corpus`);
lossy round-trip at `render.rs:387`; unvalidated sync ingress at
`sync.rs:222-230` → `store.rs:476-494,659-663,682-695`.

</details>

### ⚪ LOW · Two unescaped separators in the recipe frontmatter

**`crates/store/src/render.rs:319`** · _bug_

`esc` escapes backslash, newline and pipe only. The equipment frontmatter list
joins on `,` and the ingredient pantry link is delimited by `] `, and neither
separator is escaped. Confirmed: `equipment: vec!["a,b"]` round-trips to
`["a","b"]`; `IngredientDoc { text: "flour", pantry: Some("a] b") }` round-trips
to `{ text: "b] flour", pantry: Some("a") }`. `tag_esc` correctly escapes its own
`;` and `=` separators, so the render layer has three separator alphabets and
only one of them is handled.

- **Spec:** `CLAUDE.md`, *The export never lies*; `render.rs:6-8`.
- **Suggested fix:** type `RecipeDoc.equipment` as `Vec<Slug>` and
  `IngredientDoc.pantry` as `Option<Slug>` so the separators are unrepresentable
  (slug-ness is already enforced downstream in
  `clean_equipment`/`clean_ingredients` — this just moves it into the type).
  Failing that, extend escaping to `,` in list context and `]` in `esc`, with
  matching unescape.

<details><summary>Verification trail — code pointers</summary>

Confirmed by round-trip. `render.rs:319` (equipment join), the ingredient
delimiter in the same function, `esc` and `tag_esc` definitions above.

</details>

---

## Store — sync & threads

**Files:** `crates/store/src/sync.rs` (the sans-IO `Peer`), `threads.rs`
(append-only rows), `crates/store/tests/{sync,convergence}.rs`
**Read first:** implementation doc → *Sync protocol*, *Log-row identity*,
*Threads are log-shaped*; `CLAUDE.md` → *CRDT convergence, concretely*
**Key entry points:** `Peer::start`, `Peer::handle`, `Peer::commit`,
`Store::insert_log_row`, `Store::insert_thread_row`
**Theme:** this is the weakest layer in the codebase, and it has one root
cause — the sync ingress path trusts the peer where the local path validates.
Everything else here follows from that, or from the absence of a round-level
transaction.

### 🔴 HIGH · Sync writes stale snapshots, silently erasing concurrent writes

**`crates/store/src/sync.rs:183`** · _bug_

`Peer::start` loads every doc into `DocPeer.doc` once, at session start, and
nothing reloads it for the life of the session. The server releases the store
mutex between rounds, so chat tool edits, `/api/edit` and `/api/revert` can
append changes meanwhile. `Peer::commit` passes that stale doc into
`append_changes`, which re-reads `MAX(seq)` from SQLite — so it *knows* about the
other writer's rows — but when `seq % 64 == 0` it writes `doc.save()` of the
stale doc as the snapshot for that `upto_seq`. `load_doc` then starts from that
snapshot and replays only `seq > upto_seq`, so every concurrent change at or
below the boundary becomes invisible to reads, exports and future syncs. There
is no error: Automerge merely buffers the dependent changes. Two overlapping
WebSocket sessions trigger it with no chat involved.

- **Spec:** implementation doc, *Sync*: "Every round is persisted before replying, so an interrupted sync loses nothing"; `CLAUDE.md`: "Converged replicas also produce byte-identical exports."
- **Suggested fix:** make the snapshot authoritative — when one is due, rebuild
  or load the doc *inside* the same `append_changes` transaction instead of
  saving the caller's copy (or have `Peer::commit` reload before appending). The
  underlying contract mismatch is worth fixing in the signature too:
  `append_changes`' doc comment promises only "doc must already contain the
  changes", which is weaker than what a snapshot needs. Regression test: open a
  WS session, `/api/edit` mid-session, cross the 64-change boundary, assert the
  edit survives in `corpus()` and the export.

<details><summary>Verification trail — code pointers</summary>

Confirmed by tracing every link. `Peer::start` loads once at
`sync.rs:124-139`; `commit` at `:175-189` diffs against `dp.baseline` and passes
`&mut dp.doc` straight through. `append_changes` at `store.rs:257-297` computes
`seq` from `SELECT COALESCE(MAX(seq),0)` — i.e. it sees other writers' rows —
then inserts `doc.save()` of the caller's doc as the snapshot when `seq %
SNAPSHOT_EVERY == 0` (`SNAPSHOT_EVERY = 64` at `store.rs:28`). The snapshot's
contents and its `upto_seq` therefore come from two different sources. Mutex
released between rounds at `crates/server/src/lib.rs:176-185,193-196`.

</details>

### 🟠 MEDIUM · Sync accepts peer-supplied uids without verifying the content hash

**`crates/store/src/sync.rs:222`** · _security_

`Peer::handle` feeds `row.uid` and `row.entry` straight into
`insert_log_row`/`insert_thread_row`, both `INSERT OR IGNORE` keyed on the uid
unique index, with no recomputation of `sha256(content)[..16]`. Since the uid is
the *entire* cross-replica identity, a mismatched uid lets a peer suppress a row
(send junk under the real row's uid; the genuine entry is later swallowed by `OR
IGNORE` and filtered out of `missing` forever) or duplicate one (send the real
entry under a bogus uid). `append_log` compounds it: the occurrence index is a
`LIKE`-count that assumes dense indices, and the bool returned by
`insert_log_row` is discarded, so a colliding uid silently writes nothing while
`append_log` still returns `Ok(uid)` and performs the first-cook promotion.

- **Spec:** implementation doc, *Log-row identity*: "uid = `sha256(entry)[..16]-<n>`"; *Threads are log-shaped*.
- **Suggested fix:** validate on the sync insert path — split the uid at the
  last `-`, recompute the content hash, and reject the round (`Corrupt`) on
  mismatch or a non-integer suffix. Separately, make `append_log` propagate a
  failed insert instead of returning a uid it did not write.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `sync.rs:222-231` does exactly `store.insert_log_row(&row.uid,
&row.entry)?` on peer-supplied values; the hash helpers are never called from
`sync.rs`, only from `append_log` (`store.rs:453-462`) and the thread append
(`:670-678`). `INSERT OR IGNORE` at `store.rs:476-494,682-695`; the round's
`missing` computation filters purely on the peer's uid set at `sync.rs:234-253`.
Trust note: this is a single-user system with one static token, so the peer is
not a hostile party in the normal case — the realistic trigger is a buggy or
differently-versioned client, which is why this is medium rather than high.

</details>

### 🟠 MEDIUM · A sync round is persisted doc-by-doc with no round-level transaction or validation

**`crates/store/src/sync.rs:175`** · _bug_

`Peer::commit` loops over docs calling `ensure_doc_row` (outside any
transaction) then `append_changes` (its own transaction per doc). There is no
round-level transaction and no check that the resulting doc set hydrates. Since
`corpus()` requires every location with a pantry doc to also have equipment,
shops and fridge, one missing sibling — from a kill between per-doc
transactions, a mid-migration client, or a partial round — makes `corpus()` fail
and takes down every read: `/api/queue`, `/api/pages`, `mise queue`, `mise
export`, and context assembly for every chat exchange. The server's post-sync
export failure is only `warn!`'d, so the first symptom is a dead API.

- **Spec:** `sync.rs` module doc: "Everything received is persisted after each round, so an interrupted sync loses nothing."
- **Suggested fix:** wrap the round's persistence in one transaction so it is
  all-or-nothing; validate that affected docs hydrate before committing
  (`WireMsg::Error` otherwise); and make `corpus()` degrade for an incomplete
  location rather than failing the whole read.

<details><summary>Verification trail — code pointers</summary>

Confirmed, with the poisoning reproduced. `ensure_doc_row` is a bare
`conn.execute("INSERT OR IGNORE INTO docs …")` with no transaction
(`store.rs:300-306`); `append_changes` opens and commits its own transaction per
doc (`:263`, `:295`). `handle` at `sync.rs:204-232` accepts a sync message for
any parseable `DocId`, creating a fresh `AutoCommit` for ids it has never seen,
and calls `commit` unconditionally at `:232`. `corpus()`'s all-or-nothing shape
at `store.rs:765-776`; the swallowed export failure at
`crates/server/src/lib.rs:226-229`.

</details>

### 🟠 MEDIUM · Interrupted sync and malformed peer input are untested

**`crates/store/tests/sync.rs:32`** · _test-gap_

`run_sync` and the server's `client_sync` always drive clean sessions to
completion. Nothing drops a session after round N, rebuilds the `Peer`s and
asserts convergence with no lost or duplicated rows — which is exactly the
invariant the stale-snapshot and per-doc-transaction defects above break.
Separately, the server parses peer-controlled JSON with zero coverage for
invalid doc ids, non-base64 data, garbage Automerge bytes, uids that do not
match their content, repeated `log_uids`, or a doc id creating a location
without siblings. All are reachable branches.

- **Spec:** implementation doc, *Sync protocol*; `CLAUDE.md`, *CRDT convergence* (seeded interleavings, partition/merge).
- **Suggested fix:** a property test that cuts a session at a seeded round
  index, rebuilds both `Peer`s from the stores, and asserts convergence plus no
  duplicated or lost rows; plus a table of hostile `WireMsg` inputs asserting
  each is rejected without mutating the store.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `tests/sync.rs:32-47,317-347`;
`crates/server/tests/sync_ws.rs:36-64`; the untested branches at
`sync.rs:191-284`.

</details>

### 🟠 MEDIUM · The named basement-checkoff test does not run through the real interfaces

**`crates/store/tests/convergence.rs:361`** · _test-gap_

`basement_checkoff_merges_with_desktop_pantry_edit` forks two raw `AutoCommit`
documents and merges them both ways, asserting a property of Automerge's map
merge rather than of *this system*: no SQLite persistence, no `append_changes`
dedupe, no snapshot cadence, no `Peer` rounds, no export. The honest-interface
equivalents in `tests/sync.rs` never touch the shopping list at all — the
property's `Op` enum is `Pantry|Queue|Log|Thread` with no `Shopping` variant. So
the most-cited scenario in the charter is never driven through the real path,
which is why the `s1` collision defect (below, in tools) survives a green suite.

- **Spec:** `CLAUDE.md`, *Testing*: the offline shopping-list scenario "gets an explicit named test", modelled through the honest interface.
- **Suggested fix:** rewrite the named test against two real `Store`s using the
  `run_sync` helper — B offline checks off and adds shopping items while A edits
  the pantry — asserting both corpus equality and byte-identical exports. Add a
  `Shopping` variant to the reconvergence property's `Op` enum.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `convergence.rs:360-401` (the named test on bare
`AutoCommit`s); `tests/sync.rs:32-47,233-239` (the honest-interface property and
its `Op` enum).

</details>

### ⚪ LOW · Sync inserts thread rows without the normalization the renderer assumes

**`crates/store/src/store.rs:682`** · _bug (uncertain)_

`append_thread_message` normalizes CRLF→LF, trims, and refuses empty content
before hashing and inserting. `Peer::handle` calls `insert_thread_row` directly
with a `ThreadMessage` deserialized off the wire, applying none of it.
`render::thread_page` documents its dependence on the invariant, and the content
hash is taken over the normalized form locally and the raw form on sync — so
identical turns get different uids across replicas.

- **Spec:** `render.rs:377`: "Content is normalized on append (LF, trimmed, non-empty)."
- **Suggested fix:** normalize and validate inside `insert_thread_row` (or a
  shared `ThreadMessage::new` used by both paths) and reject a round whose
  message normalizes to empty. This is the same fix as the export-generator
  finding above.

<details><summary>Verification trail — code pointers</summary>

Marked **uncertain** by the verifier. The literal code claim is confirmed:
`append_thread_message` normalizes at `store.rs:652-679`, `insert_thread_row` is
a bare `INSERT OR IGNORE` at `:682-695`, and `Peer::handle` feeds it wire data
at `sync.rs:227-231` with no validating deserialize. What could not be confirmed
is whether any in-tree client actually produces CRLF or untrimmed thread content
today — both current writers normalize before sending — so the *reachable*
consequence depends on a future or third-party client. Kept because the
invariant is asserted in a doc comment that the code does not enforce.

</details>

### ⚪ LOW · The occurrence-index uid is replica-local, so partitioned repeat cooks under-count

**`crates/store/src/store.rs:453`** · _spec-drift_

`append_log` derives the occurrence index with `COUNT(*) WHERE uid LIKE
prefix||'-%'`, with no replica component. Two replicas that each log the same
content N and M times converge to `max(N,M)` rows rather than N+M. The doc
promises both halves — cross-device dedupe of one cook *and* distinct `-0`/`-1`
for genuine repeats — and the second half fails whenever the repeats straddle a
partition. Threads have the same shape. The existing test
`same_cook_logged_on_both_devices_dedupes` picks the one arrangement that works
(2 vs 1 → 2 rows).

- **Spec:** implementation doc, *Log-row identity*: "The same cook logged on two devices merges to one row; a genuinely repeated identical cook is `-0`, `-1`."
- **Suggested fix:** decide which promise wins. Either state the limitation in
  the doc, or make the suffix replica-scoped
  (`sha256(entry)[..16]-<replica-id>-<n>` with a per-corpus random id). Add the
  symmetric-repeat case to the test either way.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `store.rs:453` (the `LIKE` count), the thread equivalent at
`:670-678`, and the asymmetric test in `tests/sync.rs`.

</details>

### ⚪ LOW · `pending_entries`/`pending_threads` are orphan state, and `reply_empty` omits a field

**`crates/store/src/sync.rs:118`** · _quality_

Both vectors are set and taken within a single `handle()` invocation, so they
are locals masquerading as protocol state. `reply_empty` checks `docs`,
`log_uids`, `log_entries` and `thread_entries` but omits `thread_uids` — correct
only because the two uid lists are always assigned together, which a comment
asserts but nothing enforces.

- **Suggested fix:** make the two vectors locals in `handle()`, and compute
  `reply_empty` from the constructed `Round` itself (a `Round::is_empty()` that
  checks every field, so a new field cannot be forgotten).

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `sync.rs:118` (the fields), the `reply_empty` computation and
the comment that stands in for the invariant.

</details>

---

## Assistant — Anthropic client

**Files:** `crates/assistant/src/client.rs` (hand-rolled Messages API client,
SSE framer, turn assembler)
**Read first:** implementation doc → *Anthropic client*, *Risks: API drift*
**Key entry points:** `AnthropicClient::next_turn`, `SseFrames::push`,
`Assembler::handle`, `Assembler::finish`
**Theme:** the framer is byte-naive where the TypeScript mirror is not, and the
failure policy is inconsistent — unknown events are tolerated while unknown
block types are fatal, and truncation is fatal where the driver expects
graceful degradation.

### 🟠 MEDIUM · The SSE framer corrupts multi-byte characters split across chunks

**`crates/assistant/src/client.rs:180`** · _bug_

`SseFrames::push` does `buf.push_str(&String::from_utf8_lossy(chunk))` with no
undecoded byte tail, so any chunk ending mid-sequence yields U+FFFD and the raw
bytes are discarded before reaching the buffer. The corpus is full of
sauté/crème/æøå and the project's own em dashes, and the streamed JSON carries
them raw. The damage is not cosmetic: corrupted text lands in the assembled
`ContentBlock::Text`, is stored via `append_thread_message`, and is written to
the markdown export. Inside a `tool_use` `partial_json` it can silently corrupt
a tool argument (U+FFFD keeps the JSON parseable) or, when the split hits a
structural byte, break the parse and abort the exchange. The TypeScript mirror
gets this right with `TextDecoder({stream:true})` — the Rust side, described in
the docs as the reference implementation, is the one that drifted.

- **Spec:** implementation doc, *Anthropic client*: "incremental framer"; "a TS SSE framer mirroring the Rust one"; `CLAUDE.md`, *The export never lies*.
- **Suggested fix:** make `SseFrames` byte-oriented — keep a `Vec<u8>`, scan for
  `b"\n\n"`, and decode only complete frames (a frame boundary is always a char
  boundary). Extend the existing byte-at-a-time test with multi-byte fixtures;
  today it feeds one byte at a time but uses an ASCII-only payload, which is why
  it passes.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `SseFrames` holds only `buf: String` (`client.rs:173-176`); `push` at
`:180`; the chunk source at `:80-89`. The TS mirror creates `new TextDecoder()`
and calls `decoder.decode(value, { stream: true })` in `web/src/lib/api.ts`.

</details>

### 🟠 MEDIUM · A `max_tokens` truncation mid tool-call fails the whole exchange

**`crates/assistant/src/client.rs:316`** · _bug_

`Assembler::finish` unconditionally parses each tool block's accumulated
`input_json`. When the model is cut off mid `input_json_delta` the string is a
JSON prefix, `from_str` fails, and `next_turn` returns `Api("tool input didn't
parse")` — discarding the stop reason *and* the completed text blocks. The turn
driver documents the opposite behaviour ("max_tokens mid-call yields what text
we have rather than a half-executed round"), and that branch is unreachable
because the client errors before `absorb` ever sees the turn. `MAX_TOKENS` is a
hardcoded 8192 and the tool set has long-argument tools, so this is reachable in
ordinary use.

- **Spec:** `crates/assistant/src/turn.rs:111-113` — max_tokens mid-call yields what text we have.
- **Suggested fix:** when `stop == MaxTokens` and a tool block's `input_json`
  fails to parse, drop that block and keep the text so `absorb` returns
  `Step::Done`; keep the hard error under other stop reasons. Unit-test a
  truncated `partial_json` turn.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `finish` at `client.rs:306-332` collects with `?` and parses
unconditionally; `stop` is only consulted afterwards at `:328`. The
`input_json_delta` arm blindly appends at `:268-272` and `content_block_stop` is
an explicit no-op at `:300-302`, so nothing prevents a truncated block reaching
`finish`. Both callers propagate with `?` before `Turn::absorb` runs
(`exchange.rs:68`, `server/src/chat.rs:98`).

</details>

### 🟠 MEDIUM · No timeouts and no retry on the model client

**`crates/assistant/src/client.rs:32`** · _bug_

`AnthropicClient::new` builds `reqwest::Client::new()`: no connect timeout, no
read timeout, no overall timeout, no TCP keepalive. A blackholed connection (NAT
expiry, uplink flap, laptop suspend) means `stream.next().await` never resolves
*or* errors — on the server the detached drive task and its SSE response hang
forever with no `done`/`error` event; on the CLI `mise chat` hangs silently. The
sibling `fetch_url` client sets a 20 s timeout as documented hardening; the
model client, which holds a user-facing stream open, has none. Separately, the
non-2xx path turns 429 and 529 into an immediate hard error with no retry and no
`retry-after` — after the user's message has already been appended to the
thread.

- **Suggested fix:** build the client once with `connect_timeout`, `read_timeout`
  (which bounds the gap between chunks) and `tcp_keepalive`; retry the initial
  POST on 429/529/5xx with bounded backoff honouring `retry-after`, but only
  before any delta has been emitted. Hoist the client out of per-exchange
  construction while you are there.

<details><summary>Verification trail — code pointers</summary>

Confirmed, with one sub-claim corrected. `client.rs:30-37` builds a stock
client; the only timeout configured anywhere in the workspace is
`fetch.rs:107`. The streaming loop at `:78-90` is an unbounded `while let
Some(chunk) = stream.next().await`. The non-2xx path at `:68-76` maps every
status to an immediate error. (The finding's aside about the CLI hanging
"silently" is right; the server-side task leak is bounded by process lifetime,
not unbounded growth.)

</details>

### ⚪ LOW · The framer only recognizes `\n\n`, so a CRLF endpoint yields zero frames

**`crates/assistant/src/client.rs:182`** · _bug_

SSE permits CRLF, LF or lone-CR line endings, and this client is explicitly
designed to be pointed elsewhere (`with_base_url`, `--anthropic-base-url` for
proxies and the E2E fake). Against a CRLF stream, `find("\n\n")` never matches:
the buffer grows for the whole response with nothing drained (there is no size
cap even for well-formed streams) and the caller gets "stream ended without a
stop reason" — an error that blames the model for a framing problem. Secondary:
`raw.lines()` strips `\n` but not `\r`, and data is only `trim_start()`ed, so a
trailing `\r` is retained and embedded mid-payload for multi-line data fields.

- **Spec:** implementation doc, *Anthropic client*: "streaming SSE with an incremental framer."
- **Suggested fix:** normalize line endings on ingest (or search for the
  earliest of `\n\n`, `\r\n\r\n`, `\r\r`) and `trim_end_matches('\r')` on field
  values; add a CRLF framer test and bound the buffer size.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `client.rs:182` (the `find`), the `lines()` handling below it, and
the absence of any buffer cap.

</details>

### ⚪ LOW · Prompt caching leaves the message tail uncached across tool rounds

**`crates/assistant/src/client.rs:118`** · _quality_

`request_body` marks `cache_control: ephemeral` on the system block and the last
tool definition; nothing marks the message tail. The driver loops up to
`MAX_TOOL_ROUNDS = 32`, re-sending every prior assistant turn and tool result
(queue views, page bodies, fetched pages), so exchange cost is quadratic in tool
rounds over the tail. The code comment frames this as intentional because "the
tail is what varies" — but *within* an exchange the tail is append-only, which
makes it exactly what is worth caching.

- **Spec:** implementation doc, *Anthropic client*: "cache_control markers on the system block and last tool."
- **Suggested fix:** add a `cache_control` marker to the last content block of
  the final message (still two breakpoints, under the limit); verify
  `cache_read_input_tokens` rises across rounds, and update the doc if the
  policy changes. Cost, not correctness.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `client.rs:118` (`request_body`'s markers), `turn.rs`
(`MAX_TOOL_ROUNDS`).

</details>

### ⚪ LOW · Unknown SSE events are tolerated but unknown block types are fatal

**`crates/assistant/src/client.rs:253`** · _architecture_

`Assembler::handle` ends with `_ => Ok(None)`, tolerating any unknown event
name, while `content_block_start` returns `Err` on an unrecognized block type
and `content_block_delta` does the same for an unrecognized delta (a future
`citations_delta`, say). Both policies cannot be right. The fatal path takes
down live user exchanges rather than an eval run, while the doc's API-drift
stance leans on evals noticing drift. It is defensible as a deliberate tripwire
— but then it should say so and be consistent.

- **Spec:** implementation doc, *Risks — API drift*: "keep the surface minimal, and keep evals runnable so drift is noticed."
- **Suggested fix:** pick one policy and document it. Skip-and-warn for unknown
  block/delta types (pushing a placeholder so block indices stay aligned), with
  hard errors reserved for structurally broken data such as a `tool_use` with no
  id.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `client.rs:253` (the tolerant catch-all), the fatal arms in
`content_block_start` and `content_block_delta`.

</details>

---

## Assistant — turn, exchange, context

**Files:** `crates/assistant/src/{turn,exchange,context,seam,error}.rs`
**Read first:** implementation doc → *The seam, concretely*, *Context assembly*
**Key entry points:** `Turn::absorb`, `Turn::provide`, `run_exchange`,
`context::assemble`
**Theme:** the sans-IO state machine is clean, but its edges — an empty reply, a
mid-loop abort, a photo attached by position rather than by identity — are
under-specified, and every one of them fails toward "looks like success".

### 🟠 MEDIUM · Photos are attached to whichever thread message sorts last

**`crates/assistant/src/exchange.rs:58`** · _bug_

`run_exchange` appends the user turn, calls `context::assemble` (which reads
thread messages `ORDER BY created, uid`), then attaches the image blocks to
`history.last_mut()`. Nothing enforces that the message just appended is the
last one: `append_thread_message` stamps the caller-supplied civil `DateTime`
verbatim, and thread rows merge across replicas by uid union, so a message with
a later civil timestamp (a phone in a timezone ahead, or with a fast clock) can
already exist. If the last message is an assistant turn, `Image` blocks land in a
`ChatRole::Assistant` message, which the Messages API rejects with a 400; if it
is an older user turn, the photos are silently attached to a stale question. The
server has the identical bug, and every test uses a fresh store with a monotonic
clock.

- **Spec:** implementation doc, *Context assembly*: "History is the thread's text turns"; `CLAUDE.md`, *Time is an input*.
- **Suggested fix:** stop inferring the target from ordering — thread the
  appended uid through (or build the outgoing user turn locally from the prior
  history), and assert `last.role == User` before splicing. Regression test:
  seed an assistant message stamped in the future.

<details><summary>Verification trail — code pointers</summary>

Confirmed; no guard exists. `exchange.rs:56-62`; `thread_messages` ordering at
`store.rs:739-750`; verbatim civil stamp at `store.rs:652-678` and the sync
union at `sync.rs:228`. The only monotonicity clamp in either driver
(`exchange.rs:96-99`, `chat.rs:136-139`) covers reply-after-question, not
user-message-is-last.

</details>

### 🟠 MEDIUM · A truncated turn with no text is reported as a successful empty reply

**`crates/assistant/src/exchange.rs:93`** · _bug_

`Turn::absorb` ends the exchange whenever `stop != ToolUse`, returning
`Step::Done(reply)`. If the model was cut off by `max_tokens` mid `tool_use`
before emitting any text, `reply` is empty: `run_exchange` skips
`append_thread_message` and returns `Ok` with an empty reply, the server emits
`done {"reply":""}`, and the CLI prints a bare newline and exits 0. A truncated
turn is indistinguishable from a successful empty answer, the thread is left
ending on a dangling user message, and tool mutations from earlier rounds stay
applied.

- **Spec:** implementation doc, *The seam* — the turn driver is the exchange's contract; a truncated turn must not be reported as completed.
- **Suggested fix:** distinguish the cases in `absorb` — on `MaxTokens` (or a
  turn with neither text nor executable calls) return an error or a distinct
  `Step`. At minimum treat an empty reply as a failure in `run_exchange` and
  `chat::exchange`. Add turn and exchange tests.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `turn.rs:114-116` returns `Step::Done(self.reply.clone())` whenever
`turn.stop != ToolUse || calls.is_empty()`, and `self.reply` only accumulates
non-empty trimmed text (`:86-93`). `MaxTokens` is genuinely produced from the
wire (`client.rs:290`) under the 8192 cap (`client.rs:20`). Persistence gated on
a non-empty reply at `exchange.rs:93-102`.

</details>

### 🟠 MEDIUM · An aborted exchange leaves store mutations unexported and the thread dangling

**`crates/assistant/src/exchange.rs:85`** · _bug_

Any `?` inside the tool loop — a `StoreError` from `tools::execute`, an API error
from `next_turn`, or the `MAX_TOOL_ROUNDS` protocol error — propagates out of
`run_exchange` after the user message has been persisted and earlier tool calls
have already mutated the store. Every caller exports only on the success path,
so on abort the markdown does not reflect changes the tools already made, and
the thread keeps a question with no answer and no record of failure. The round
cap in particular throws away accumulated narration. The export self-heals on
the next successful mutation, but between those points the readable backup is
behind the store — and only the pre-model failure path is tested.

- **Spec:** `CLAUDE.md`, *The export never lies*; implementation doc, *Tools*: "No export inside tools; one export per exchange."
- **Suggested fix:** export on the error path too (best-effort, with a "chat
  (failed)" provenance) before propagating, and record a short assistant turn or
  marker so the thread is not left dangling. For the round cap, consider ending
  with `Step::Done(reply)` plus a note rather than an error.

<details><summary>Verification trail — code pointers</summary>

Confirmed. User turn persisted at `exchange.rs:56` before the loop; the loop
propagates at `:68` (`next_turn`), `:69` (`absorb`) and `:85` (`tools::execute`).
Store errors genuinely abort rather than becoming error tool results —
`tools.rs:54-87` maps only `NotFound/Exists/Invalid/BadDocId` to `Fail::User`.
Success-only exports at `cli/src/main.rs:575` and `server/src/chat.rs:150`.

</details>

### ⚪ LOW · `Turn::provide` accepts an empty outcome list with no round outstanding

**`crates/assistant/src/turn.rs:129`** · _bug_

`provide` validates only that outcome ids equal pending ids. With `pending`
empty, `provide(vec![])` succeeds and unconditionally pushes
`ChatMessage { role: User, content: vec![] }`, which the Messages API rejects —
surfacing as an opaque 400 on the *next* `next_turn` rather than a protocol
error at the point of misuse. Not reachable from the two in-tree drivers, but
`Turn` is the crate's public sans-IO surface whose entire purpose is that
callers shuttle turns and outcomes themselves.

- **Spec:** implementation doc, *The seam*: "The tool loop is a sans-IO `Turn` state machine"; `CLAUDE.md`, correct by construction.
- **Suggested fix:** track the outstanding round explicitly (`pending:
  Option<Vec<ToolCall>>` or a state enum) and return `Protocol("provide called
  with no tool round outstanding")`. Unit-test it alongside
  `outcomes_must_match_pending_calls`.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `turn.rs:129` (the validation and the unconditional push).

</details>

### ⚪ LOW · The reply stamp is clamped against its question, not the thread

**`crates/assistant/src/exchange.rs:91`** · _bug_

Both drivers stamp the user turn with `clock().datetime()` unclamped and clamp
only the reply against `now`, which is local civil time. Thread order is
`(created, uid)`, driving both the export transcript and the history fed back to
the model, so any backwards movement of civil time — a DST fall-back, an NTP
step — places a whole exchange before earlier ones. Replicas still converge and
export identically, so this does not break the two-devices promise; the
transcript simply reads out of order, and the assistant is handed a scrambled
conversation on resume.

- **Spec:** implementation doc M3: "Ordering is (created, uid); the reply … clamped monotone so transcripts sort in conversation order."
- **Suggested fix:** clamp the user turn too, against the thread's current
  `MAX(created)` (expose it on `Store`), stamping `max(now, last + 1ns)`; share
  one helper between `run_exchange` and the server's exchange rather than
  keeping two copies.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `exchange.rs:91` and `chat.rs:136-139` (the reply-only clamps); the
unclamped user stamp above each.

</details>

### ⚪ LOW · `assemble` renders the entire corpus to extract three or four pages

**`crates/assistant/src/context.rs:89`** · _quality_

`assemble` calls `render(&store.corpus()?)`, which loads every document and
renders state, queue, someday, shopping, steering, facts, every location page,
every recipe, technique, log month and thread transcript — then uses at most
four entries. Thread transcripts in particular are rendered in full and dropped,
so cost grows with conversation history that is never used. On the server this
runs inside the store mutex, blocking sync sessions for the duration.

- **Spec:** implementation doc, *Context assembly*: "slow-moving corpus context (state/steering/facts, plus the page for page threads)."
- **Suggested fix:** render only what the prompt needs — a
  `render_one(&CorpusState, &DocId)` helper, or per-doc render functions keyed
  off `DocId::export_path()` (which is already the single authority for that
  mapping).

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `context.rs:89` (the full render), `:96-114` (what is actually
consumed).

</details>

---

## Assistant — tools & views

**Files:** `crates/assistant/src/tools.rs` (the ~19 deterministic operations),
`views.rs` (the one structured queue view behind both renderings)
**Read first:** implementation doc → *Tools*; design doc → *Editing & trust
model*
**Key entry points:** `tools::dispatch`, `tools::parse`, `ToolCtx::msg`,
`views::queue_view`
**Theme:** the tool layer treats model output as well-formed. It is not — it is
generated text — and the gaps between "well-formed" and "valid" are where these
findings live: unknown fields dropped, unknown tiers accepted, ids allocated
positionally, provenance interpolated raw.

### 🔴 HIGH · Shopping and fridge ids are allocated positionally, so concurrent adds destroy each other

**`crates/assistant/src/tools.rs:1131`** · _bug_

`shopping_add` and `fridge_add` allocate ids by scanning for the lowest free
`s<n>`/`p<n>` in the **local** replica's map. Two partitioned replicas both
allocate `s1`; the merge resolves the conflicting `put_object` deterministically
to one value, and the other item is gone from every replica and from the export,
silently, with no conflict surfaced. Confirmed empirically against the real
`ShoppingDoc`/autosurgeon stack: two forks adding "milk" and "eggs" merge to
`{"s1": milk}`. The allocator also reuses ids after removal, so `fridge_remove
p1` on one device can delete a different portion another device created as `p1`.
`append_log`, `append_thread_message` and `queue_add` all derive content-based
identities; these two regressed to a positional counter.

- **Spec:** `CLAUDE.md`, *CRDT convergence*: the offline shopping-list scenario; implementation doc: tap-shaped endpoints so the M9 offline queue is a replay buffer.
- **Suggested fix:** give shopping items and fridge portions replica-safe
  identities — content-hash + occurrence like the log, a device-prefixed id, or
  a ULID. Then extend the convergence property to allocate ids through the real
  tool path and assert item *count* is preserved, not merely that replicas
  agree; today it models adds into a fixed small key space (`s{k%6}`) and only
  asserts agreement, so a converged-but-lossy state passes.

<details><summary>Verification trail — code pointers</summary>

Confirmed by reproduction. `shopping_add` at `tools.rs:1129-1135`
(`(1..).map(|n| format!("s{n}")).find(|c| !d.items.contains_key(c))`);
`fridge_add` at `:1002-1010` with `p{n}` and no caller-supplied-id escape hatch
at all. `ShoppingDoc.items` is a `BTreeMap<String, ShoppingItemDoc>`
(`pages.rs:102-105`) reconciled into Automerge by autosurgeon, so the generated
id *is* the map key and two concurrent puts at that key conflict. The weak
property is at `convergence.rs:192-198,237-246`.

</details>

### 🟠 MEDIUM · Tool inputs ignore unknown fields, so a model typo succeeds silently

**`crates/assistant/src/tools.rs:95`** · _bug_

`parse::<T>()` uses `serde_json::from_value` into structs with no
`deny_unknown_fields`, and the schemas' `additionalProperties: false` is
documentation rather than a gate — nothing validates the incoming `Value`
against the schema before dispatch. This is worse than a no-op because the edit
tools have defaults: `pantry_set` *creates* a missing item with presence
"have", so a dropped `presence` field asserts the opposite of what the user said
and still returns "pantry home: eggs updated". `shopping_update` has the same
shape: with neither `done` nor `remove` given it changes nothing and answers
"shopping: updated s1".

- **Spec:** implementation doc, *Tools*: model-recoverable problems (bad input, unknown slug, duplicate) return as `is_error` results.
- **Suggested fix:** add `#[serde(deny_unknown_fields)]` to every tool input
  struct — enforced at the single `parse` choke point — so unknown keys become
  `is_error` and the model can correct itself. Make `shopping_update` reject a
  call specifying neither `done` nor `remove`.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `parse<T>` at `tools.rs:94-96` is the single deserialization point;
`grep -rn "deny_unknown_fields" crates/` returns zero hits. `obj()` at
`:152-158` emits `additionalProperties: false` into the schema only. `pantry_set`
at `:840-887` requires only `item`.

</details>

### 🟠 MEDIUM · Any well-formed slug is accepted as a source tier

**`crates/assistant/src/tools.rs:857`** · _bug_

`pantry_set` and `shopping_add` validate `tier` only as a slug, never checking
it exists in the location's shops page — even though `resolve_location` right
there demonstrates the existence-check pattern and the tool descriptions say
"see the location's shops page". Downstream, `Readiness::verdict` treats an
unknown tier exactly like a missing one: `ordinal == None` short-circuits the
whole trip to `NeedsShopping { tier: None }`, rendered as "source unknown". So
one typo'd tier on one item erases the tier for *every* dish that needs it,
silently and permanently. An unknown tier slug is the definition of "unknown
slug", which the error policy says must be `is_error`.

- **Spec:** implementation doc, *Tools*: unknown slug returns as an `is_error` tool result.
- **Suggested fix:** load the location's `ShopsDoc` in both tools and return
  `user("no tier X at LOC; tiers are …")` on a miss, matching `resolve_location`
  and `queue_add`'s recipe check.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `tools.rs:857` (`pantry_set`) and `:1124` (`shopping_add`) slug-check
only; `resolve_location` at `:133-148` shows the pattern. Tiers are enumerable
at call time via `DocId::Shops(Slug)` → `ShopsDoc { tiers: Vec<TierDoc> }`
(`pages.rs:207-220`). The short-circuit is at `readiness.rs:100-113`, rendered
at `views.rs:166-173`.

</details>

### 🟠 MEDIUM · `list_pages` hides recipe status from the assistant

**`crates/assistant/src/tools.rs:524`** · _spec-drift_

The annotation is `title [tags] (effort)` — no status. `queue_status` carries
none either, and nothing else in the assistant surfaces it, so the model's only
way to learn a recipe's status is `read_page` per recipe, which it will not do
when planning across a repertoire. `/api/pages` *does* include status, so the
human browse surface honours the design while the assistant's does not. Status
has real semantics: drafts stay out of rotation until a first cook, and retired
means out of rotation and out of the browse surface.

- **Spec:** design doc:132-135 (retired = out of rotation and the browse surface); implementation doc:205-209 (drafts stay out of rotation).
- **Suggested fix:** include status in the `list_pages` annotation, or omit
  non-active recipes by default with a flag to include them. Test that a retired
  recipe is distinguishable.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `tools.rs:513-526` (the annotation); `server/src/api.rs:88-93`
(the JSON API, which does include status).

</details>

### 🟠 MEDIUM · One dangling recipe reference takes down the entire queue view

**`crates/assistant/src/views.rs:153`** · _architecture_

`dish_view` does `store.get::<RecipeDoc>(...)?` with no fallback and
`queue_view` propagates, so a single dangling reference fails the whole view —
the tool returns an error instead of the queue and `/api/queue` 404s. An
unparseable slug produces `Corrupt`, which the tool layer classifies as ours and
aborts the exchange outright. Referential integrity between the queue doc and
recipe docs is not enforceable by construction (they are separate Automerge
documents), `queue_add` checks existence only at insert time, and the sync
design explicitly allows a replica whose queue doc has caught up while a recipe
doc has not. The queue is the home screen; the right failure mode is one
degraded row.

- **Spec:** implementation doc, *Tools*: `queue_status` with readiness/coverage is the primary read.
- **Suggested fix:** degrade per dish — emit a `DishView` with the slug, no
  effort, and a "recipe missing" verdict instead of failing the view. Test that
  a queue entry referencing an absent recipe still renders the rest.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `views.rs:91-112,142-155`; `server/src/api.rs:53-57` (the 404);
`sync.rs:8-13` (the design note allowing the skew).

</details>

### ⚪ LOW · Provenance messages interpolate model text unescaped and unbounded

**`crates/assistant/src/tools.rs:47`** · _security_

`ToolCtx::msg` builds `"{provenance}: {action}"` with raw model input embedded
(shopping add `{text}`, log `{title}`, fridge add `{dish}`, pantry remove
`{item}`); nothing strips newlines or caps length. That string is the change
message the trust model displays as "who changed this page". A model — or
injected content arriving via `fetch_url` — can produce a history line that
reads like a UI-provenance entry once the browser collapses the newline, and
unbounded text rides into every replica's immutable change history.

- **Spec:** design doc, *Editing & trust model*: "Every page shows recent changes (what, when, from which conversation)."
- **Suggested fix:** normalize in `ToolCtx::msg` — strip control characters and
  newlines, truncate to a bounded length — as the export commit paths already do
  for their summaries.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `tools.rs:47` (`ToolCtx::msg`) and the call sites that embed model
strings. Immutability is the aggravating factor: change history only grows.

</details>

### ⚪ LOW · `equipment_set` blanks an existing note when `note` is omitted

**`crates/assistant/src/tools.rs:936`** · _bug_

It does `e.items.insert(item, note.unwrap_or_default())`, so calling it without
`note` clears the existing note. `pantry_set` is documented as "Only the fields
you pass change" and implements an entry-and-patch; `equipment_set`'s own
description reads as the same contract, and the tool is named `_set` rather than
`_add`.

- **Spec:** implementation doc, *Tools*: the pantry/equipment pair documented with "Only the fields you pass change."
- **Suggested fix:** patch rather than replace — assign the note only when
  present, keeping an explicit empty string as the clear operation.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `tools.rs:936` (the unconditional insert), contrasted with
`pantry_set`'s entry-and-patch at `:865-887`.

</details>

### ⚪ LOW · `queue_add` on an existing id resets its age and drops sibling dishes

**`crates/assistant/src/tools.rs:610`** · _bug_

`queue_add` always writes a fresh `QueueEntryDoc { dishes: vec![one], reason,
added: today }`. The description says "Upserts by id" but not that the upsert
resets age or drops sibling dishes — and there is no other tool to amend an
entry, so the model's only way to add a reason is to re-call `queue_add`. Age is
load-bearing: `queue_view` computes `age_days` and `render_queue_status` prints
"21d on the queue" precisely so stale entries are noticeable. Multi-dish entries
render as menus, and they get collapsed to one.

- **Spec:** implementation doc, *Tools*: `queue_status` reports the queue with readiness annotations; `age_days` is part of the view contract.
- **Suggested fix:** preserve `added` and the dish list when the id exists, or
  add a narrow `queue_update` for reason-only edits and let `queue_add` refuse an
  existing id with an `is_error`.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `tools.rs:610` (the unconditional fresh entry); `views.rs` computes
`age_days` from `added`.

</details>

### ⚪ LOW · `slugify` drops non-ASCII, so non-Latin titles fail with "invalid slug"

**`crates/assistant/src/tools.rs:580`** · _bug_

`slugify` keeps only `[a-z0-9]`, turning other runs into hyphens then trimming.
A title with no ASCII alphanumerics yields `""` and `Slug::new("")` fails.
Accented Latin titles silently mangle ("Crème brûlée" → `cr-me-br-l-e`) and that
becomes the entry id in the queue export. Verified on the CLI: `mise queue add
"寿司"` → `Error: invalid slug ""`, with no hint that passing an explicit id would
work. For a cookbook whose corpus is full of transliterations, non-ASCII dish
names are the normal case, not an edge one.

- **Suggested fix:** transliterate before filtering (`deunicode`/`unidecode`) or
  fall back to a stable derived id (a short content hash of the title) when
  slugification comes out empty; name the id parameter in the error message
  either way.

<details><summary>Verification trail — code pointers</summary>

Confirmed on the CLI. `tools.rs:580` (`slugify`); `Slug::new` rejects the empty
string.

</details>

### ⚪ LOW · No test asserts that tool edits record provenance

**`crates/assistant/tests/tools.rs:29`** · _test-gap_

`tests/tools.rs` builds a `ToolCtx` with provenance "planning thread" and then
only asserts hydrated doc state; nothing calls `Store::history` to check the
change message. `tests/exchange.rs` likewise checks doc state and thread
messages but not history. The only provenance assertion in the repo is at the
store layer with a hand-written message, which cannot catch a tool that forgets
`ctx.msg(...)`. Since every mutating tool threads `ctx.msg` by hand, a
copy-paste that drops it would pass the whole suite — and provenance is the
trust model's central invariant.

- **Spec:** design doc, *Editing & trust model*; implementation doc:465 ("Every assistant edit records provenance").
- **Suggested fix:** one test looping a representative mutation per doc kind,
  asserting `store.history(&doc)` ends with a message starting with the ctx
  provenance prefix and carrying `ctx.at()`.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `tests/tools.rs:29` and the absence of any `history` call in
the assistant tests.

</details>

---

## Assistant — fetch & recon

**Files:** `crates/assistant/src/fetch.rs` (the `Fetch` seam and `HttpFetch`
guard), `extract.rs` (JSON-LD → Readability → Markdown), `recon.rs` (photo
validation, proposal parsing)
**Read first:** implementation doc → *`fetch_url` tool*, *`Fetch` is a seam like
`Model`*, *M6 recon decisions*
**Key entry points:** `validate_url`, `HttpFetch::fetch`, `extract::extract`,
`recon::parse_proposal`, `Photo::validate`
**Theme:** the SSRF guard is textual where it needs to be numeric, and the
extraction pipeline is unbounded in the one dimension the size cap does not
cover — depth. The recon validators are sound but their limits disagree with the
transport's.

### 🔴 HIGH · IPv4-mapped IPv6 literals bypass the private-address refusal

**`crates/assistant/src/fetch.rs:67`** · _security_

The `Ipv6` arm rejects only `::1`, `::`, `fc00::/7` and `fe80::/10`.
`::ffff:127.0.0.1` has `segments()[0] == 0`, so `is_loopback()` is false and both
mask checks pass — and since the host is a literal there is no DNS step, so the
v4-mapped `SocketAddrV6` reaches the IPv4 stack directly. Same for
`::ffff:10.0.0.1`, `::ffff:169.254.169.254` (the cloud metadata address) and the
deprecated `[::127.0.0.1]`. Decimal and octal IPv4 forms are normalized by the
`url` crate and *are* caught, which is what makes the gap easy to miss. The same
closure guards every redirect hop, so a public URL can 302 into it.

- **Spec:** implementation doc, *`Fetch` is a seam*: "private addresses and local hostnames refused" on every redirect hop.
- **Suggested fix:** in the `Ipv6` arm, map via
  `to_ipv4_mapped().or_else(to_ipv4)` and run the IPv4 predicate first; also
  consider refusing `64:ff9b::/96`, `100.64.0.0/10` and `0.0.0.0/8`. Add these
  literals to the existing rejection test table — the failing test is one line
  each.

<details><summary>Verification trail — code pointers</summary>

Confirmed by execution. An in-file probe run with `cargo test -p mise-assistant
--lib fetch::tests -- --nocapture` showed the `url` crate parsing these as
`Host::Ipv6` with validation returning `Ok(())`:
`http://[::ffff:127.0.0.1]/x`, `http://[::ffff:10.0.0.1]/x`,
`http://[::127.0.0.1]/x` (host `::7f00:1`), `http://[::ffff:169.254.169.254]/x`.
The equivalent IPv4 literals that the test table at `fetch.rs:154-177` asserts
are refused all slip through in v4-mapped form. Guard at `:67-77`, redirect
closure at `:96-104`.

</details>

### 🟠 MEDIUM · Hostnames are checked textually, so any name resolving into a private range is fetched

**`crates/assistant/src/fetch.rs:50`** · _security_

A `Host::Domain` is refused only for the literal suffixes `localhost`,
`.localhost`, `.local` and `.internal`. A public name whose A/AAAA record points
at `127.0.0.1` or `10.x` passes, and reqwest then resolves and connects — so
`http://127.0.0.1.nip.io:8080/` reads a loopback service straight into the
model's context. The same resolve-then-connect window exists for redirect hops,
which are validated by name. Note the module comment already disclaims being a
bulletproof SSRF boundary while the implementation doc states the control
without that caveat, so code and doc disagree about what is guaranteed.

- **Spec:** implementation doc, *`Fetch` is a seam*: "private addresses and local hostnames refused."
- **Suggested fix:** resolve the host yourself and validate every resolved
  `IpAddr` before connecting (a custom resolver/connector, or request against
  the pinned IP with a `Host` header) — or soften the doc to the honest
  guarantee and make the code comment agree. This is a decision for the user,
  since it is a real complexity cost for a personal deployment.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `validate_url` at `fetch.rs:43-79` handles `Host::Domain` at `:50-60`
with four literal suffix checks and no resolution. `HttpFetch::new` at `:95-112`
builds a stock reqwest client whose redirect policy calls the same name-based
check at `:96-104`. No custom resolver, connector interception or IP pinning
exists anywhere in the crate.

</details>

### 🟠 MEDIUM · Readability extraction is unbounded in DOM depth and runs on the async runtime

**`crates/assistant/src/extract.rs:170`** · _bug_

`execute_fetch` awaits the network then calls the pure `extract::extract`
synchronously inside the async fn, so `Readability::parse` runs on the worker
driving the chat exchange with no time budget and no `spawn_blocking`. Measured
on the pinned `dom_smoothie` 0.18 (release build) with one paragraph inside N
nested divs: depth 500 / 6.9 KB = 0.62 s; 1000 = 1.2 s; 2000 / 23 KB = 7.7 s;
4000 / 45 KB = 55 s; 8000 / 90 KB = still running after four minutes. A flat 2 MB
page is fine (85 ms) — so the 2 MB cap bounds *bytes* but not *work*. The 20 s
reqwest budget covers only the network, and the server spawns `chat::drive`
detached, so a client disconnect does not cancel the burn.

- **Spec:** implementation doc, *`fetch_url` tool*: "Size cap, timeout, http(s) only, private ranges blocked" — the timeout covers only the fetch.
- **Suggested fix:** run extraction under a deadline off the runtime —
  `timeout(spawn_blocking(|| extract(...)))`, erroring back to the model on
  timeout — and cap DOM depth before handing the document to Readability.

<details><summary>Verification trail — code pointers</summary>

Confirmed, with the timings reproduced independently by the verifier.
`execute_fetch` at `fetch.rs:34` (`.and_then(|html| extract::extract(&html,
url))`, inline); `readable_article` at `extract.rs:169-178` with no bound; the
only timeout at `fetch.rs:107`; `MAX_HTML` applied while streaming at
`fetch.rs:85,126-132`. `grep spawn_blocking crates/` returns nothing; both call
sites `.await` inline (`exchange.rs:77`, `server/src/chat.rs:109`).

</details>

### 🟠 MEDIUM · ISO-8601 durations with days or seconds render as "0 min"

**`crates/assistant/src/extract.rs:90`** · _bug_

`duration` parses into a `jiff::Span` and reads only `get_hours()` and
`get_minutes()`. jiff spans are unbalanced by construction — they retain exactly
the units written — so `P2D` → "0 min", `P1DT2H` → "2 h" with the day vanished,
`PT45S` → "0 min". schema.org `totalTime` in days is ordinary for anything
fermented, brined, cured, chilled overnight, or sourdough. The rendered fact
line goes into the model's context and from there into a drafted recipe page,
where a fabricated "0 min" reads as authoritative rather than missing.

- **Spec:** implementation doc, *`fetch_url` tool*: schema.org Recipe JSON-LD rendered faithfully.
- **Suggested fix:** include days and seconds, or convert with
  `span.total(Unit::Minute)` and format from that. Add `P2D`, `P1DT2H` and
  `PT45S` fixtures.

<details><summary>Verification trail — code pointers</summary>

Confirmed by compiling a throwaway test against the workspace's jiff 0.2.35:
`P2D` → days=2 hours=0 min=0 (renders "0 min"); `P1DT2H` → days=1 hours=2
(renders "2 h"); `PT45S` → sec=45 (renders "0 min"). `duration()` at
`extract.rs:85-96`; consumed at `:133-137`.

</details>

### 🟠 MEDIUM · The two security-critical fetch behaviours have no tests

**`crates/assistant/src/fetch.rs:96`** · _test-gap_

The redirect re-validation is an anonymous closure inside `HttpFetch::new` and
the byte cap lives inside `HttpFetch::fetch` — both need real network IO, which
the suite correctly never does, and the exchange tests drive `fetch_url` through
a `ScriptedFetch` above this layer. The only test,
`url_policy_rejects_the_obvious`, covers `validate_url` in isolation and its
table is exactly the set of cases that already pass — which is what let the
IPv4-mapped hole through. Neither `attempt.previous().len() > 5` nor the
`bytes.len() > MAX_HTML` break is structurally guaranteed.

- **Spec:** implementation doc, *`Fetch` is a seam*: "`HttpFetch` re-validates every redirect hop … 20 s budget, 2 MB cap."
- **Suggested fix:** extract the decision into a pure `fn redirect_ok(next,
  hops) -> Result<(), String>` and unit-test it (public→private hop, sixth hop,
  cross-scheme); likewise make the cap a pure accumulate step, or test `fetch`
  against a loopback test server with an injected policy.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `fetch.rs:96-104` (the closure), `:126-132` (the cap),
`:141-164` (the one test); `tests/exchange.rs:179-249` (the scripted layer that
bypasses both).

</details>

### ⚪ LOW · An empty JSON-LD Recipe husk beats the real article

**`crates/assistant/src/extract.rs:19`** · _bug_

`extract` takes the JSON-LD branch whenever `json_ld_recipe` returns `Some`, and
that returns on the first Recipe-typed object in the first parseable `ld+json`
block. `render_recipe` never checks for substance: `{"@type":"Recipe","name":"X"}`
renders `# X`, which is non-empty, so the emptiness guard does not fire and
`readable_article` is never consulted. Roundup pages and server-rendered shells
that emit a Recipe stub therefore come back as a bare heading even though the
substance was right there on the page.

- **Spec:** implementation doc, *`fetch_url` tool*: JSON-LD when present, else Readability extraction.
- **Suggested fix:** treat a Recipe object with neither `recipeIngredient` nor
  `recipeInstructions` as no match — keep scanning the remaining `ld+json`
  blocks and fall through to `readable_article`. Add a husk-plus-article
  fixture.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `extract.rs:19` (the branch), `render_recipe` and its emptiness
guard.

</details>

### ⚪ LOW · The response charset is ignored

**`crates/assistant/src/fetch.rs:133`** · _bug_

`fetch` accumulates raw bytes for the 2 MB cap and then calls
`String::from_utf8_lossy`, discarding both the `Content-Type` charset and any
`<meta charset>`. reqwest's own `Response::text()` honours the charset; the
manual streaming loop loses it. windows-1252 / ISO-8859-1 pages — still common
on regional recipe sites — turn every non-ASCII byte into U+FFFD, and the
corruption flows into the JSON-LD parse (which then fails and silently falls
back to Readability) and into the drafted page.

- **Suggested fix:** capture the charset from `Content-Type` before streaming
  and decode the collected bytes with `encoding_rs` (already in reqwest's
  dependency tree), falling back to a `<meta charset>` sniff on the first chunk.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `fetch.rs:133` (`from_utf8_lossy` over the accumulated bytes).

</details>

### ⚪ LOW · The /chat body limit sits below recon's own combined cap

**`crates/server/src/lib.rs:78`** · _bug_

The route caps the body at 12 MiB while `MAX_PHOTOS = 12`, `MAX_DATA =
8,000,000` base64 chars each, and `MAX_TOTAL = 20,000,000`. Every recon limit is
*above* the transport limit, so `validate_all`'s friendly branches ("too many
photos", "too large together") can never fire over HTTP, and the comment
claiming "the Photo validator enforces the real ceiling" is false. `photo.ts`
downscales to roughly 0.5–1 MB base64 per frame, so a real 12-frame shelf recon
lands at 6–12 MB and can trip an opaque 413. The recon message is reachable only
from the CLI.

- **Spec:** implementation doc M6: "A recon carries as many frames as the shelf needs, all in one exchange" — the frame budget should be one number.
- **Suggested fix:** derive the route's `DefaultBodyLimit` from
  `recon::MAX_TOTAL` plus JSON overhead (or lower `MAX_TOTAL`/`MAX_PHOTOS` under
  the transport limit), and correct the comment to name the authoritative
  number.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `server/src/lib.rs:78-81` (12 MiB); `recon.rs:24-37` (the three
constants and `validate_all`).

</details>

### ⚪ LOW · `MAX_DATA` admits images above the Messages API's per-image ceiling

**`crates/assistant/src/recon.rs:24`** · _bug_

`MAX_DATA` is 8,000,000 base64 characters ("~6 MB decoded"), above the API's 5 MB
per-image limit, so anything in between passes local validation and dies at the
model call as an opaque API error instead of a friendly validation message. The
web client downscales so it never gets close — but the CLI base64-encodes files
straight from disk with no downscaling, which is exactly where a raw phone photo
enters. Because the user turn is appended before the model call, the failure
also leaves a "[photo attached]" turn with no reply.

- **Suggested fix:** set `MAX_DATA` from the API limit (~5 MiB × 4/3 base64
  chars) and say so in the comment, so the local "downscale before upload" error
  is the one users actually see.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `recon.rs:24` (the constant and its comment); the CLI's
encode-from-disk path.

</details>

### ⚪ LOW · A proposal's location is never checked to exist

**`crates/assistant/src/recon.rs:135`** · _bug_

`parse_proposal` only requires `Slug::new` to succeed (recon has no store access
by design), and the server parks the proposal in `AppState::proposals` without
checking either. `pantry_set`'s `resolve_location` then rejects the unknown
location, so every Apply tap 400s — and `annotate_proposal` only annotates when
the proposal's location equals the active one, so `completed` stays false
forever and the entry is never dropped from the map.

- **Spec:** implementation doc M6: "Each proposal line is exactly one `pantry-set` tap on the existing edit endpoints."
- **Suggested fix:** validate location existence in `chat.rs` before inserting
  into `state.proposals`, turning a miss into an error tool outcome ("no location
  X") so the model retries with the right one.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `recon.rs:135` (`parse_proposal`'s slug-only check); the server's
insert into `AppState::proposals`; `annotate_proposal`'s active-location
condition.

</details>

### ⚪ LOW · `fetch_url` will fetch any URL the model emits

**`crates/assistant/src/fetch.rs:43`** · _security_

`validate_url` enforces scheme and non-private host only; the rule that fetches
must be *user-initiated* lives entirely in prose the model can be talked out of
(the tool description and the system prompt). Fetched page text re-enters the
same conversation as a tool result, and the system prompt already contains
state, steering and facts verbatim — so injected instructions sit alongside the
household's private facts and can reach them, with an attacker-chosen query
string as the channel. Nothing rate-limits or logs the URLs a turn fetched.
Impact is bounded (a personal cookbook, no credentials in the corpus), but the
mitigation is a code-level check the code does not have.

- **Spec:** implementation doc, *`fetch_url` tool*: "One deliberate URL at a time"; the tool description: "Only for URLs the user explicitly gave you."
- **Suggested fix:** enforce the rule in code — require the URL's origin and
  path to appear in one of the thread's user-authored messages before fetching,
  returning a model-facing error otherwise, and cap fetches per exchange.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `fetch.rs:43` (`validate_url`'s complete set of checks); the
user-initiated rule appears only in the tool description and system prompt.

</details>

---

## Server

**Files:** `crates/server/src/lib.rs` (router, auth gate, WS sync),
`api.rs` (the JSON API and edit allowlist), `chat.rs` (SSE streaming),
`main.rs` (startup, shutdown)
**Read first:** implementation doc → *Auth*, *The JSON API*,
*`/api/edit/{action}`*, *A proposal lives until completed or superseded*
**Key entry points:** `app()`, `authorized()`, `chat_endpoint`, `api::revert`,
`api::edit`
**Theme:** every finding here traces to one structural choice — auth is a
hand-rolled call at the top of ten handler bodies instead of a layer. That
placement is what puts it after body buffering, what makes the route list
untestable, and what spreads the `?token=` fallback to routes that never needed
it.

### 🟠 MEDIUM · Auth runs after the body is buffered and deserialized

**`crates/server/src/lib.rs:123`** · _security_

Axum completes all extractors before the handler body runs. `chat_endpoint`
declares `Json<ChatRequest>` last and only then calls `authorized()`, and /chat
raises the body limit to 12 MiB. So an unauthenticated request is fully read,
parsed by `serde_json`, and re-materialized into `Vec<ChatImage>` `String`s
(roughly doubling the allocation) before the 401 is written. `api::edit` and
`api::revert` have the same shape at the 2 MB default. There is no rate limiting
and the service is proxied from the public internet. Structurally, auth being
the first statement of ten handlers rather than a layer means any route added
later is unauthenticated by default.

- **Spec:** implementation doc, *Auth* / *The JSON API*: "Bearer-authed under `/api`."
- **Suggested fix:** move the check into `middleware::from_fn_with_state` (or a
  tower layer) applied to the authed routes so it runs on the request parts
  before any body extractor; layer `/health` and the static fallback separately
  and delete the in-handler checks. This also makes the route-coverage test
  below derivable rather than hand-maintained.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `chat_endpoint` at `lib.rs:123-131` with `Json<ChatRequest>` as the
final argument and `authorized()` as the first statement at `:129`; `ChatImage`
fields at `:112-117`; the raised limit at `:78-81`. Same shape at
`api.rs:223-231` and `:261-272`.

</details>

### 🟠 MEDIUM · The bearer token is accepted from a query parameter on every endpoint

**`crates/server/src/lib.rs:156`** · _security_

The `?token=` fallback lives in the single gate used by every handler, though
only `ws_sync` has a genuine excuse (browsers cannot set headers on a WebSocket
handshake). No in-repo client uses it — the web app always sends the
`Authorization` header and the CLI sets it explicitly — so on the HTTP routes it
is pure attack surface. Tokens in URLs land in reverse-proxy access logs (the
module puts Caddy in front), browser history, and `Referer` headers on outbound
links — and this app renders outbound links from third-party-derived content,
including through the unsanitized `{@html}` sink. The token is the whole
authorization model: static, and effectively unrevocable short of editing the
NixOS config.

- **Spec:** implementation doc, *The JSON API*: "Bearer-authed under `/api`."
- **Suggested fix:** restrict the query fallback to the WebSocket upgrade
  handler (a separate `authorized_ws`) and make the shared `authorized()`
  header-only; add a `Referrer-Policy: no-referrer` on the static app.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `authorized()` at `lib.rs:151-158` falls back to
`query.get("token")`, and it is the gate for the chat SSE endpoint (`:129`), the
WS upgrade (`:166`) and all ten `/api` handlers (`api.rs:50, 65, 121, 170, 193,
229, 271, 325`). Client usage: `web/src/lib/api.ts:34-42` (header only; the web
app opens no WebSocket at all), `crates/cli/src/remote.rs:78` (header).

</details>

### 🟠 MEDIUM · `/api/revert` panics on a multi-byte hash

**`crates/server/src/api.rs:234`** · _bug_

`&request.hash[..request.hash.len().min(8)]` byte-slices a `String` straight off
the wire; `min(8)` bounds length, not char boundaries. The slice runs *before*
`Store::revert` would reject the value as "not a change hash", so the downstream
validation never gets a chance. It happens while the store mutex guard is held
— tokio's `Mutex` does not poison, so the process survives — but there is no
`CatchPanicLayer`, so the connection is dropped with no response and a panic is
logged where a 400 belonged.

- **Spec:** implementation doc, *The JSON API*: malformed input is the caller's error (`fail` maps `Invalid`/`BadDocId` to 400).
- **Suggested fix:** validate the hash first (hex, expected length), or build
  the short form with `request.hash.chars().take(8).collect::<String>()`.
  Regression test: `POST /api/revert` with `{"doc":"queue","hash":"€€€"}` expects
  400.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `api.rs:234` (the byte slice); `RevertRequest` at `:217-221` is a
bare `Deserialize` with no validation; ordering confirmed — `DocId::parse` at
`:233` succeeds, the slice at `:234` runs before `store.revert` at `:236`, so
`store.rs:580-582`'s `Invalid` mapping and `fail()`'s 400 at `api.rs:29-38` are
unreachable. `grep` for `CatchPanic` across the repo returns zero hits.

</details>

### 🟠 MEDIUM · The auth-coverage test misses every mutating route

**`crates/server/tests/api.rs:198`** · _test-gap_

`threads_and_auth` loops over `api/queue`, `api/pages`, `api/page/queue`,
`api/history/queue` and `api/thread/planning`. Absent: `/api/revert` (mutates the
corpus), `/api/edit/{action}` (mutates the corpus), and `/api/location` (leaks
the full pantry/equipment/fridge view). The `?token=` fallback is never asserted
on a mutating route. `/chat` and `/sync` *do* have negative auth tests, which
makes the omission read as drift rather than a deliberate scope. Because auth is
one hand-rolled call per handler with no middleware, the test enumerates a
hard-coded list rather than deriving it from the router — so every new handler
silently adds an untested one.

- **Spec:** implementation doc, *Auth*: applies to the whole API surface, mutations included.
- **Suggested fix:** turn the loop into a table over (method, path, body)
  covering every route including the POSTs, with both missing-header and
  wrong-token cases. Once auth is a layer, derive the list from a shared route
  table used by both `app()` and the test.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `tests/api.rs:189-209` (the five-route loop); the uncovered
handlers at `api.rs:116-131,223-243,271-272`.

</details>

### 🟠 MEDIUM · Graceful shutdown never runs under systemd

**`crates/server/src/main.rs:133`** · _bug_

`with_graceful_shutdown` waits on `tokio::signal::ctrl_c` — SIGINT only.
systemd's default `KillSignal` is SIGTERM and `nix/module.nix` sets no override,
so with no SIGTERM handler installed the kernel default terminates the process
immediately: the shutdown future is dead code in the only supported deployment.
In-flight sync WebSockets and /chat SSE streams are severed mid-frame. The worst
case is a restart landing inside `Store::export`, which rewrites and removes
files before running `git add`/`commit` — leaving the readable backup
half-rewritten and uncommitted.

- **Spec:** implementation doc, *Server defaults*: the hardened systemd unit is the supported runtime.
- **Suggested fix:** `select!` over `ctrl_c` and
  `signal(SignalKind::terminate())`, cfg-gated for non-Unix; optionally add
  `TimeoutStopSec` once the drain is real. The tokio `signal` feature is already
  enabled.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `main.rs:133-135` awaits only `ctrl_c()`. A repo-wide grep for
`SignalKind|signal::unix|SIGTERM|KillSignal|TimeoutStopSec` matches nothing
outside those lines: `nix/module.nix:86-111` sets `ExecStart`, `User/Group`,
`StateDirectory`, `LoadCredential`, `Restart`/`RestartSec` and hardening
options, but no `KillSignal`. Export's non-atomic file phase at
`store.rs:830-857`.

</details>

---

## CLI & remote

**Files:** `crates/cli/src/main.rs` (the `mise` binary — 1043 lines),
`remote.rs` (join, sync transport, `remote.json`)
**Read first:** implementation doc → *Server defaults* (join flow, `remote.json`
0600), *Surfaces* (`mise chat`)
**Key entry points:** `run()`'s subcommand match, `show_queue`, `remote::sync`,
`remote::save`, `normalize_url`
**Theme:** the CLI is where the export promise is actually broken, and it is
also where the codebase's copy-paste drift is most visible — a whole
reimplementation of the queue renderer, plus several guards that cannot fire.

### 🔴 HIGH · A thread-only sync never exports, so synced transcripts exist only in SQLite

**`crates/cli/src/main.rs:400`** · _bug_

The post-sync export is gated on `!outcome.docs_updated.is_empty() ||
outcome.log_added > 0`, omitting `outcome.threads_added` — even though
`SyncOutcome` carries it, `remote::describe` reports it, and the server's
equivalent guard includes it. Thread transcripts render to `threads/<id>.md` and
are covered by the export determinism and completeness properties. Since
`Store::export` is already a no-op when nothing changed, the guard buys nothing
and costs the invariant. Reproduced end to end: the row lands in `mise.db` with
no `export/threads/` directory, and repeated `mise sync` prints "already in sync"
and never exports — so the gap is permanent until an unrelated local mutation
happens to trigger an export.

- **Spec:** `CLAUDE.md`, *The export never lies*: no state that exists only in SQLite.
- **Suggested fix:** drop the guard and always export after a successful sync,
  or at minimum add `|| outcome.threads_added > 0` to match the server.
  Regression test: sync a corpus whose only incoming item is a thread message and
  assert `export/threads/planning.md` exists and matches the sender's.

<details><summary>Verification trail — code pointers</summary>

Confirmed by tracing all four pointers. Guard at `cli/src/main.rs:399-402`;
`SyncOutcome.threads_added` exists and is populated at `sync.rs:97-100`, with
thread messages travelling as their own `ThreadRow` exchange separate from the
Automerge docs (so a thread-only session leaves `docs_updated` empty and
`log_added` at 0); the server's correct guard at
`crates/server/src/lib.rs:218`.

</details>

### 🟠 MEDIUM · An interrupted sync exports nothing, and neither does the next one

**`crates/cli/src/main.rs:399`** · _bug_

`remote::sync` persists every round as it goes — that is the documented
"interrupted sync loses nothing" property — but `let outcome =
remote::sync(...)?;` aborts the command on any transport or peer error, so
`store.export` is never reached even though changes were committed. The server
breaks out of its loop and exports regardless of why the loop ended. Combined
with the threads-only guard above, the next `mise sync` then reports "already in
sync" and also skips the export, so the export never catches up.

- **Spec:** implementation doc, *Sync protocol*: "Every round is persisted before replying, so an interrupted sync loses nothing."
- **Suggested fix:** have `remote::sync` return the outcome alongside the error
  (or export in a cleanup step) so the CLI exports whatever was persisted before
  re-raising, mirroring the server's post-loop export.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `main.rs:399` (`?` returns before the export at `:400-402`).
`remote.rs:86-98` propagates with `?` in three places while rounds are already
committed — and its own error context string reads "connection lost mid-sync
(already-received data is saved)", confirming the persist-as-you-go design that
the caller then discards. A peer-reported error after successful rounds is a
normal reachable path (`sync.rs:193-195`).

</details>

### 🟠 MEDIUM · The CLI hand-copies the queue renderer, and the copies have already drifted

**`crates/cli/src/main.rs:858`** · _architecture_

`show_queue` (`:858-921`) and `dish_line` (`:926-970`) reproduce
`views::render_queue_status` (`views.rs:218-274`) and `views::dish_line`
(`:188-215`) statement for statement: same sort key, age filter, verdict
strings, unlinked pluralisation, freezer note, someday rendering. `views.rs`
documents itself as "one structured view, two renderings"; the CLI is an
undeclared third. The copies already diverge on the empty-queue line, and
nothing in the suite compares the two renderings. `mise-assistant` is already a
dependency of the CLI, so this is deletion, not plumbing.

- **Spec:** implementation doc, *The JSON API*: "one structured type in `mise-assistant::views` that both the tool string and the JSON render from."
- **Suggested fix:** delete `show_queue`/`dish_line` and call
  `views::queue_view` + `views::render_queue_status`; make the CLI-specific
  empty-queue hint a parameter rather than a fork.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `cli/src/main.rs:858-970` against `assistant/src/views.rs:82-280`;
`crates/cli/Cargo.toml` already lists `mise-assistant`.

</details>

### 🟠 MEDIUM · `remote.json`'s 0600 mode is applied only on creation

**`crates/cli/src/remote.rs:35`** · _security_

`OpenOptions::mode()` is passed to `open(2)` and honoured only when `O_CREAT`
actually creates the file. `save()` uses
`create(true).truncate(true).mode(0o600)`, so an existing 0644 file — restored
from a backup, copied with `cp`/`rsync`, written by an older build, or chmod'd —
keeps its mode while being rewritten in place with the bearer token in
cleartext. Reproduced: `chmod 644` then `mise remote set … --token …` leaves
`-rw-r--r--`. Both the code comment and the docs state the 0600 guarantee
unconditionally.

- **Spec:** implementation doc, *Auth*: "Clients store the token in `remote.json` (0600) beside the corpus — never in the export."
- **Suggested fix:** after writing, unconditionally `set_permissions(0o600)` —
  or write to a temp file with 0600 and rename over the target, which also makes
  the write atomic. Test both a fresh save and a rewrite over 0644.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `remote.rs:32-44` (the `OpenOptions` chain); `grep -rn
set_permissions --include=*.rs` returns nothing, and there is no temp-file+rename
path, so the write is neither atomic nor mode-normalizing.

</details>

### 🟠 MEDIUM · A trailing slash in the remote URL drops the `/sync` path

**`crates/cli/src/remote.rs:59`** · _bug_

The default-path branch keys on `after_scheme.contains('/')`, which is true for
a bare trailing slash — so `https://host/` returns `wss://host` with the slash
trimmed instead of `wss://host/sync`, and the connection requests `/` rather
than the server's `/sync` route. Reproduced: `mise init --from
ws://127.0.0.1:7931/` fails with 404 while the same URL without the slash joins
fine. Trailing slashes are exactly what browsers and copy-paste produce, and the
bad value is *saved*, so every later `mise sync` fails with a 404 that points
nowhere near the cause.

- **Spec:** implementation doc, *Server defaults*: "`mise init --from <url> --token …` = bare corpus + saved remote + first sync."
- **Suggested fix:** trim trailing slashes first, then decide. Add unit tests for
  `host`, `host/`, `host/sync`, `host/sync/` — the function is pure and
  currently has none.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `remote.rs:47-64` (`normalize_url`); the server mounts the WebSocket
handler only at `/sync` (`server/src/lib.rs:72-89`). The normalized value is
persisted by `remote::save` and reused at `cli/src/main.rs:331,374,390`.

</details>

### 🟠 MEDIUM · Remote mode's documented guarantees have no tests

**`crates/cli/tests/remote.rs:108`** · _test-gap_

`tests/remote.rs` covers only the happy path (join, offline edits, converge,
third sync no-op) and the missing-remote error. Nothing asserts `remote.json`'s
mode, nothing exercises `normalize_url` (a pure function with a real defect
above), and the convergence test compares export trees only in a scenario where
doc/log changes force an export — no thread messages are ever synced, so the
thread-only export hole is invisible. Both of this region's shipped defects sit
squarely in the gap.

- **Spec:** `CLAUDE.md`, *The export never lies*; implementation doc, *Auth* (`remote.json` 0600).
- **Suggested fix:** unit tests for `normalize_url` over host / host+slash /
  host+path / host+path+slash; a test asserting 0600 after a fresh save *and*
  over an existing 0644 file; and an end-to-end case where the only synced item
  is a thread message.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `tests/remote.rs:70-124`; the untested pure functions at
`remote.rs:32-64`.

</details>

### ⚪ LOW · A push-only sync reports "already in sync"

**`crates/cli/src/remote.rs:126`** · _bug_

`SyncOutcome` counts only received docs plus log/thread added and sent — there
is no `docs_sent` — so a push-only session equals `SyncOutcome::default()` and
`describe` short-circuits to "already in sync". Reproduced after a local pantry
edit. The `parts.is_empty()` fallback is therefore unreachable dead code: any
non-default outcome sets one of the five fields, each of which pushes an entry
into `parts`.

- **Suggested fix:** track pushed changes in `SyncOutcome` (`docs_sent`,
  incremented in `Peer::generate_all` when a message carries changes the peer
  lacked) and report them; then "already in sync" is honest and the fallback
  becomes reachable or removable.

<details><summary>Verification trail — code pointers</summary>

Confirmed by reproduction. `remote.rs:126` (`describe`), `sync.rs:97-100`
(`SyncOutcome`'s fields).

</details>

### ⚪ LOW · A failed first sync leaves an unusable corpus that `init --from` refuses to retry

**`crates/cli/src/main.rs:329`** · _bug_

The join arm creates a bare store, saves `remote.json`, then syncs. A sync
failure — bad token, wrong URL (including the trailing-slash defect above),
server down — propagates, leaving a root whose `mise.db` contains no documents.
Every later command fails with "no such document: state", and re-running `init
--from` fails with "corpus already initialized". The only recovery is `mise sync
--server <url-with-path>` or deleting the directory, neither of which any error
message suggests.

- **Spec:** implementation doc, *Server defaults*: the client join flow.
- **Suggested fix:** make the join idempotent — if the root holds a bare corpus
  with no state doc, open it and retry the sync — or roll back the created
  directory on failure. At minimum, wrap the error with "joined but the first
  sync failed — fix the URL/token and run `mise sync`".

<details><summary>Verification trail — code pointers</summary>

Confirmed. `main.rs:329` (the join arm's ordering).

</details>

### ⚪ LOW · Remove subcommands report success for ids that never existed

**`crates/cli/src/main.rs:443`** · _bug_

`Store::modify` applies the closure and returns the *post*-mutation value, so
`q.entries.contains_key(&id)` and the `still_there` checks can never be true —
phantom guards — while the case that can actually go wrong (the id was absent)
is unchecked. Verified: `mise queue remove not-there` → "removed not-there";
`mise pantry remove nope` → "pantry home: nope removed"; `mise fridge remove
nope` → "fridge home: removed nope". `modify` also no-ops when nothing changed,
so no commit is made and the success message contradicts the git history.

- **Suggested fix:** capture presence inside the closure and `bail!("no such …
  {id}")` when absent; delete the unreachable post-mutation guards.

<details><summary>Verification trail — code pointers</summary>

Confirmed by running the commands. `main.rs:443` and the sibling remove arms;
`Store::modify`'s return semantics.

</details>

### ⚪ LOW · A mid-session close frame is treated as a successful sync

**`crates/cli/src/remote.rs:95`** · _bug_

The loop exits successfully on `Message::Close(_)` and on stream end without
asking whether the session converged (`Peer` signals completion only via `handle`
returning `None`). A server closing mid-session — graceful shutdown, proxy idle
timeout, load-balancer drain — produces exit 0 and a cheerful summary; combined
with the empty-outcome reporting above, a session cut off after the first round
prints "already in sync".

- **Suggested fix:** track whether the `done` handshake completed and return an
  error like "sync ended early — run it again" otherwise. Received data is
  already persisted, so only the reporting and exit status change.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `remote.rs:95` (the close-frame arm and the loop exit conditions).

</details>

---

## Web client

**Files:** `web/src/lib/components/*.svelte` (Thread, Markdown, editors,
ReconProposal, History), `web/src/lib/{api,sse,photo,types}.ts`,
`web/src/routes/**`
**Read first:** implementation doc → *The web app*, *Taps change data, never
structure*, *One representation at a time*, *The chat composer is a textarea*
**Key entry points:** `Markdown.svelte`'s `{@html}`, `Thread.send`,
`api.chat`, `photo.downscale`, `ReconProposal.applyAll`
**Theme:** the render path trusts its input, and the async paths have no
cancellation and no rollback — so a failure anywhere between tap and response
leaves the user worse off than before they tapped.

### 🔴 HIGH · Unsanitized markdown rendering yields stored XSS that steals the bearer token

**`web/src/lib/components/Markdown.svelte:41`** · _security_

`marked.parse()` passes raw HTML through (it has had no built-in sanitizer since
v8) and the result is injected with `{@html}`. The comment on line 40 — "The
export is our own render output, not third-party input" — is the false premise:
`fetch_url` ingests an arbitrary user-supplied URL, `extract.rs` copies JSON-LD
`name`/`description`/`recipeIngredient`/`recipeInstructions` and Readability
HTML→markdown verbatim into the tool result, the model writes it into
`RecipeDoc.body`/`ingredients`, `render.rs` writes the body byte-for-byte (and
`esc()` never escapes `<`), and `/api/page` hands it to `<Markdown>`. The payload
runs with access to `localStorage['mise-token']` — the single static bearer
token granting read, write, revert and chat over the whole corpus. Thread
transcript pages are the same sink. Even benignly, any model output containing a
tag renders as live markup, so the export and the app disagree about what the
document says.

- **Spec:** implementation doc, *The web app* (markdown via marked) and *`fetch_url` tool*, which makes third-party page content a first-class corpus input.
- **Suggested fix:** sanitize before insertion — configure `marked` to escape
  raw HTML entirely (the corpus is authored as markdown, so no page needs inline
  HTML) or run the output through bundled DOMPurify. Add a Content-Security-Policy
  on the static responses, and consider moving the token out of localStorage.
  Replace the comment on line 40 with the real reasoning, since the comment is
  what made this survive review.

<details><summary>Verification trail — code pointers</summary>

Confirmed end to end. Sink: `Markdown.svelte:23` (`marked.parse(parts.body, {
async: false })`) and `:41` (`{@html html}`); `marked` is imported in exactly one
place, there is no `marked.use(...)` anywhere, and `package.json:20` lists only
`"marked": "^18.0.7"` — no DOMPurify, no sanitizer. Source: `extract.rs:41-51`
and `:126-160` write remote page content into markdown; `render.rs:21-33` (`esc`
handles backslash, newline and pipe — not `<`), `:293-302`, `:336-342` write the
body verbatim. Transport: `api.ts:15-22` → `page/[...path]/+page.svelte:120`.

</details>

### 🔴 HIGH · A failed send destroys the draft and the attached photos

**`web/src/lib/components/Thread.svelte:55`** · _bug_

`send()` sets `draft = ''` and `photoFiles = []` before `chat()` does anything,
and pushes an optimistic user bubble. The `catch` only sets `error` — nothing
restores the draft or the photos, and nothing removes the bubble (`reload()`
sits *after* `chat()` inside the try). So the typed message is gone, the
camera-captured photos are gone — not repeatable on a phone, at a shelf — and
the transcript keeps showing a turn the server never appended, since `chat.rs`
validates photos before `append_thread_message`. The client also does not mirror
the server's photo caps, so an over-large recon is fully downscaled, uploaded,
rejected, and then discarded by this same path. The design names bad-signal shop
basements as the motivating environment.

- **Spec:** design doc:448 (store mode's photo capture and offline tolerance); implementation doc, *A recon takes multiple photos per exchange*.
- **Suggested fix:** snapshot `draft` and `photoFiles` into locals, clear only
  after `chat()` resolves, and on failure restore both and drop or mark the
  optimistic message. Enforce the 12-photo and combined-size caps in `pickPhoto`
  before doing the downscale work.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `Thread.svelte:52-54` clears the draft before any network work;
`:65` clears `photoFiles` right after `downscale` and before `await chat(...)` at
`:74`; the `catch` at `:88-89` and `finally` at `:90-94` touch only `busy`,
`streaming`, `toolNotes`. The optimistic push at `:73` precedes `chat()` and
`reload()` — the only thing that would correct it — is inside the try.

</details>

### 🟠 MEDIUM · In-flight chat streams are never cancelled on navigation

**`web/src/lib/api.ts:105`** · _bug_

`chat()` loops on `reader.read()` with no cancellation path — no
`AbortController`, no `reader.cancel()`, not even in a `finally`. `Thread.svelte`
reuses one component instance across thread changes, and the thread-change
effect resets `proposal` and reloads messages but does nothing about the
in-flight call, whose closures still hold this component's state: `onDelta`
appends to `streaming`, `onTool` to `toolNotes`, `onProposal` sets `proposal`,
and completion reloads and fires `onExchangeDone` against the *new* thread. So
sending a shelf photo on the pantry page and tapping through to a recipe page
mid-stream shows the recipe page a spinning assistant bubble of pantry text, a
`propose_pantry_diff` tool note, and a recon proposal card whose Apply buttons
write to the pantry from an unrelated page. Secondary leak: a `JSON.parse` throw
on a malformed frame escapes `chat()` with the reader never cancelled.

- **Spec:** implementation doc, *A recon proposal lives until completed or superseded* — proposal state is per-thread.
- **Suggested fix:** add an optional `signal` to `chat()`, pass it to `fetch` and
  cancel the reader in a `finally`; hold an `AbortController` per exchange in
  `Thread`, abort on thread change and unmount, reset
  `busy`/`streaming`/`toolNotes`/`error` there, and guard callbacks with a
  generation counter.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `api.ts:105-135` — no `AbortSignal` parameter, a bare `for(;;)` with
`break` only on `done`, no `try/finally`, no `reader.cancel()`.
`Thread.svelte:40-44` is the entire thread-change reaction (`void thread;
proposal = null; reload().catch(...)`) and touches none of the streaming state.

</details>

### 🟠 MEDIUM · The Edit toggle appears on non-active locations and blanks the page

**`web/src/routes/page/[...path]/+page.svelte:113`** · _bug_

`editorLocation` is derived purely from the doc path, so it is non-null for every
`location/{id}/pantry|equipment` doc and the Edit/Done toggle renders for all of
them — but the editors deliberately refuse to render for a non-active location
(`editable = view?.location === location`). The branch is exclusive, so with
`editing` true the Markdown fallback is skipped and the user gets an empty region
with no explanation. Multiple locations are a real shape today: `render.rs` emits
pages for every location and `/browse` lists them.

- **Spec:** implementation doc, *One representation at a time*: pages show either the rendered export or the editor behind an Edit/Done toggle.
- **Suggested fix:** gate the toggle on editability (fetch `/api/location`, or
  lift the active location into a shared store) so Edit only appears for the
  active location; failing that, have the editors render an explanatory
  read-only note.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `+page.svelte:42-45` (path-only derivation), `:104-110` (unconditional
toggle), `:113-121` (the exclusive if/else). Editors refuse at
`PantryEditor.svelte:51,57` and `EquipmentEditor.svelte:45,51`.

</details>

### 🟠 MEDIUM · `applyAll` erases its own error banner mid-batch

**`web/src/lib/components/ReconProposal.svelte:45`** · _bug_

`apply()` opens with `error = null` and swallows its failure into `error`;
`applyAll` loops over every not-yet-done line calling `apply()`, so each
iteration wipes the previous error — and if the last line succeeds, no error
survives. There is no per-line failure marker (the failed line simply stays
without a ✓) and `apply()` returns `Promise<void>` on both paths, so the caller
cannot learn a step failed. The user sees two ✓s, one un-ticked line and no
error, believes the batch worked, and walks away with the pantry still claiming
miso is on the shelf. Separately, `apply()` is not guarded by `busy`, so a
double-tap fires two `pantry-set` calls (idempotent, but duplicate noise in the
doc history).

- **Spec:** implementation doc, *Recon proposes; the user applies* — "Each proposal line is exactly one `pantry-set` tap."
- **Suggested fix:** have `apply()` return a boolean or throw; clear `error` once
  before the loop, collect failures, and render them as persistent per-line
  markers plus a summary ("2 of 3 applied; miso failed: …"). Guard `apply()` with
  `busy`.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `ReconProposal.svelte:29-43` (`apply`, `error = null` then
catch-to-error, `Promise<void>` both ways), `:45-51` (`applyAll`'s loop), `:65`
(the `{#if error}` banner). `applied[line.item]` is set only after the awaited
call at `:38`, so a failed line keeps its Apply button but gets no marker.

</details>

### 🟠 MEDIUM · The cookbook composer is an `<input>`, not a textarea

**`web/src/routes/cookbook/+page.svelte:163`** · _spec-drift_

The rule is unqualified: Enter sends and Shift+Enter breaks lines on hardware
keyboards; on coarse-pointer devices the return key keeps making newlines and
Send sends. `Thread.svelte` implements it (textarea, `enterSends` gated on
`pointer: coarse`, `composerKeydown`, auto-grow). The cookbook drafting box is a
peer composer — same `chat()` call, same streaming and tool-note session,
multi-turn refinement — but an `<input>` inside a form submits on Enter
unconditionally in every browser, so there is no way to write a multi-line
description and no auto-grow. Its catch also reports Unauthorized as text rather
than looping back to the token gate that the same file's effect uses.

- **Spec:** implementation doc, *The chat composer is a textarea*.
- **Suggested fix:** extract the textarea + `enterSends` + auto-grow into a
  shared `Composer.svelte` used by both — which also removes the near-duplicate
  streaming block — or at minimum swap the input for a textarea with the same
  keydown handler.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `cookbook/+page.svelte:34-63,161-171` against
`Thread.svelte:104-121,169-180`.

</details>

### 🟠 MEDIUM · Most 401s do not loop back to the token prompt

**`web/src/routes/pantry/+page.svelte:7`** · _spec-drift_

The spec is "One token prompt, localStorage, 401 loops back to it."
`clearToken()` + reload exists only in `routes/+page.svelte`, `browse`,
`cookbook` and `page/[...path]`. It is missing in `pantry/` and `equipment/` —
pure redirect shells whose only fetch is `api.location()`, so a 401 leaves a dead
page with no gate and no navigation out — and in `Thread`, `History`,
`PantryEditor`, `EquipmentEditor` and `ReconProposal`, where Unauthorized falls
through into `error = String(e)`. Compounding it, the gate only length-checks the
token and never validates it against the server, so one fat-fingered character
dismisses the gate permanently and recovery requires clearing site data by hand.

- **Spec:** implementation doc, *The web app*: "One token prompt, localStorage, 401 loops back to it."
- **Suggested fix:** move the loop-back into `api.ts`'s `request()` so no call
  site can forget it (removing four copies of the catch), and validate the token
  once against a cheap authed endpoint in the gate's `save()` rather than storing
  an unverified string.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. Missing handlers at `pantry/+page.svelte:7-12`,
`equipment/+page.svelte:7-12`, `Thread.svelte:43,88-89`,
`History.svelte:21-36`, `PantryEditor.svelte:39-42`; the shared request path at
`api.ts:29-43`; the length-only gate at `+layout.svelte:9-14`.

</details>

### 🟠 MEDIUM · All photos are decoded at full resolution concurrently

**`web/src/lib/photo.ts:11`** · _bug_

`Thread.svelte` does `Promise.all(photoFiles.map(downscale))`, and each
`downscale` starts with `createImageBitmap(file)` — decoding the original before
any scaling — with `bitmap.close()` only in the `finally`. So peak memory is N
full-resolution bitmaps plus N canvases. The design makes N large: picks
accumulate until send and the server allows 12. A 12MP frame is ~48 MB as RGBA,
so twelve is ~576 MB before canvases; iOS Safari kills the tab well below that,
and the failure lands on the composer path that has already consumed the picks
(see the draft-destruction finding above). Companions: `toDataURL` is synchronous
and blocks the main thread, and produces base64 via an intermediate data URL
rather than `toBlob`.

- **Spec:** implementation doc, *A recon takes multiple photos per exchange*; *The phone is the tested shape*.
- **Suggested fix:** downscale sequentially in a `for await` loop so at most one
  full-resolution bitmap is live, prefer `canvas.toBlob` plus a streamed base64
  step over `toDataURL`, and cap the pick count client-side at 12.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `Thread.svelte:64` is literally `images = await
Promise.all(photoFiles.map(downscale));`. `photo.ts:11` starts each call with
`await createImageBitmap(file)`, with `close()` only in the `finally` at `:23`;
`:20` uses the synchronous `toDataURL` and `:21` slices off the prefix.
`pickPhoto` at `Thread.svelte:99` appends `Array.from(input.files ?? [])`
unconditionally — no cap.

</details>

### ⚪ LOW · `createImageBitmap` is called without an explicit orientation

**`web/src/lib/photo.ts:11`** · _bug (uncertain)_

EXIF orientation is the normal state of a phone photo. `createImageBitmap`
historically defaulted `imageOrientation` to `'none'`; the spec later flipped the
default and engines adopted it at different times, so relying on it is a coin
flip. Where the default is `'none'`, the canvas re-encodes the raw sensor buffer
and drops the EXIF tag, making the rotation permanent for that exchange — and
`recon.rs` validates only media type and size. The cost is silent recon quality
loss on the surface whose whole point is reading a shelf, and `downscale` has no
unit test at all — while the repo is otherwise EXIF-aware (the eval fixtures are
EXIF-stripped).

- **Spec:** implementation doc, *Pantry recon before store mode* — recon input quality is the whole point.
- **Suggested fix:** pass `{ imageOrientation: 'from-image' }` explicitly (a
  no-op where it is already the default), and add a `downscale` unit test
  covering the scale math and the orientation option.

<details><summary>Verification trail — code pointers</summary>

Marked **uncertain**. The code claim is confirmed exactly: `photo.ts:11` calls
`createImageBitmap(file)` with no options, and the canvas re-encode at `:19-21`
drops EXIF unconditionally, with no downstream compensation
(`recon.rs:50-64,73-82` checks only media type, emptiness and size). What could
not be confirmed is which default the user's actual target browsers apply today —
if they all now default to `from-image`, the fix is a no-op safeguard rather than
a bug fix. Kept because the safeguard is one argument and the failure is silent.

</details>

### ⚪ LOW · The recipe-status fetch has no catch and no ordering guard

**`web/src/routes/page/[...path]/+page.svelte:49`** · _bug_

`api.pages().then(r => status = ...)` has no `.catch`, so any rejection (401, a
network drop, a non-JSON body) becomes an unhandled promise rejection and the
status row silently never appears, leaving the page's own error state untouched.
The effect also re-runs on `recipeSlug` change without cancelling the previous
call, and `/api/pages` walks the whole corpus so it is the slower of the page's
two fetches — meaning fast navigation can paint the previous recipe's status. A
wrong status is not cosmetic: it drives the three-button status row whose taps
POST `recipe-status` for the *current* slug.

- **Spec:** implementation doc, *Taps change data, never structure* — a tap's effect must follow from what is on screen.
- **Suggested fix:** capture the slug at effect entry and discard the response if
  it no longer matches; add a `.catch` routing into the page's error state (or
  the shared 401 handler). Better still, return status on `/api/page/{path}` so
  the page needs one fetch.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `page/[...path]/+page.svelte:49` (the uncaught `.then`), the status
row it drives below.

</details>

### ⚪ LOW · Error state is set but never cleared on success

**`web/src/lib/components/Thread.svelte:43`** · _quality_

`reload().catch(e => error = String(e))` sets `error` while the success path
never resets it — in `Thread`, `History`, `PantryEditor` and `EquipmentEditor` —
and the thread-change effect does not reset it either, so an error raised on
thread A is still displayed under thread B. Only `tap()` clears it in the
editors. The page-level `reload()` gets this right (`error = null` on success),
which makes the components' omission an inconsistency rather than house style.

- **Suggested fix:** clear `error` inside each `reload()`/`load()` on success,
  and in `Thread` also reset `error`/`streaming`/`toolNotes`/`busy` in the
  thread-change effect (the same effect that needs the abort above).

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `Thread.svelte:43` and the equivalent catches in `History`,
`PantryEditor`, `EquipmentEditor`.

</details>

---

## E2E & repo tooling

**Files:** `web/e2e/*.spec.ts`, `helpers.ts`, `serve.mjs` (the scripted fake
Anthropic endpoint), `.gitignore`
**Read first:** implementation doc → *E2E*, *The phone is the tested shape*,
*Taps change data, never structure*
**Key entry points:** the overflow auto-fixture in `helpers.ts`, `serve.mjs`'s
request handler, the recon spec's position assertion
**Theme:** several assertions that exist to pin named design rules do not
actually constrain them — they resolve before the page loads, or measure after
the transient they are about has passed.

### 🟠 MEDIUM · Four negative assertions run before the page has loaded

**`web/e2e/cookbook.spec.ts:27`** · _test-gap_

`not.toBeVisible()` and `toHaveCount(0)` resolve as soon as the condition holds —
including at t=0 against an empty DOM. `cookbook.spec.ts:26-27` asserts drafting
chatter is absent from the planning thread right after `goto('/')`, when
`messages` is still `[]`; `:38-39` asserts the Drafts heading is gone before the
cookbook page has data; `:51` asserts the New-item placeholder has count 0 while
the page still renders "Loading…"; `recon.spec.ts:96-98` asserts the proposal
card is gone right after a reload, when `proposal` is still `null`. Each of these
carries a documented design rule, which is exactly what makes the vacuity
matter: reverting the server code that removes a completed proposal leaves the
suite green.

- **Spec:** implementation doc: planning shouldn't collect drafting chatter; *One representation at a time*; *A proposal lives until completed or superseded*.
- **Suggested fix:** anchor each negative on a positive that proves the async
  load finished — a known planning message, a heading from the rendered export,
  the last assistant reply after reload — before asserting the absence.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `cookbook.spec.ts:26-27,38-39,51`; `recon.spec.ts:96-98`; the
load points they race at `Thread.svelte:18,28` and
`page/[...path]/+page.svelte:10,128-130`.

</details>

### 🟠 MEDIUM · The no-remount assertion cannot detect a remount

**`web/e2e/recon.spec.ts:63`** · _test-gap_

The spec reads the Apply button's box, clicks, awaits two settle conditions, then
reads the ✓'s box and asserts `|Δy| ≤ 50`. Both measurements are taken *after*
the DOM has fully re-settled, so the transient collapse the rule is about is over
before anything is measured — wrapping `PantryEditor` in `{#key version}` would
jerk visibly in a browser and still land the ✓ at the same final y. The tolerance
is also non-discriminating: the applied row adds ~33 px above the card, so 50
neither pins "stays put" nor is robust against a taller row or different font
metrics.

- **Spec:** implementation doc, *Taps change data, never structure*: "Remount (`{#key}`) is banned as a refresh mechanism … The recon spec pins this."
- **Suggested fix:** assert what the rule is actually about — sample
  `window.scrollY` and the card's y *before* the tap and require them unchanged,
  and/or stamp a mount identity on `PantryEditor` (a `data-instance` from a
  module counter) and assert it is identical before and after the tap.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `recon.spec.ts:56-63`; the components it is meant to constrain
at `page/[...path]/+page.svelte:11-14,111-118` and
`PantryEditor.svelte:39-42,104-129`.

</details>

### ⚪ LOW · The fake Anthropic endpoint corrupts multi-byte request bodies

**`web/e2e/serve.mjs:45`** · _bug_

`body += chunk` calls `toString()` per chunk with no continuation state — no
`req.setEncoding('utf8')`, no `Buffer.concat`. Bodies here are neither small nor
ASCII: the system prompt is dense with em dashes, plus tool schemas, corpus
context, and base64 images in the recon spec. Once the body exceeds one socket
read, a boundary inside an em dash yields U+FFFD — and the fake's dispatch is
string matching over that text. The impact is a silent, misdirecting e2e flake
rather than a production bug.

- **Spec:** implementation doc, *E2E*: a deterministic scripted fake Anthropic endpoint.
- **Suggested fix:** add `req.setEncoding('utf8')`, or collect chunks and
  `Buffer.concat(...).toString('utf8')` on end; wrap the `JSON.parse` in a
  try/catch responding 400 rather than throwing out of the end handler.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `serve.mjs:45` (the string concatenation).

</details>

### ⚪ LOW · The overflow fixture excludes the failures it most often sees

**`web/e2e/helpers.ts:32`** · _quality_

The assertion trips on `scrollWidth - clientWidth !== 0` (scrollWidth is rounded
up from fractional layout), while the diagnostic keeps only elements whose
`right > limit + 1`. Anything overhanging by more than 0 and at most 1 px — a
border, a 100%-width child in a padded parent, a rounded flex gap — fails the
assertion while being excluded from the culprit list, producing exactly the
unactionable "(nothing found)" message the helper exists to prevent. Secondary:
`el.className` on an SVG element renders as `[object SVGAnimatedString]`.

- **Spec:** implementation doc, *The phone is the tested shape*: the fixture names the offending elements on failure.
- **Suggested fix:** filter on `right > limit` (or `+0.5`), sort culprits by
  `right` descending before slicing, and use `getAttribute('class')` so SVG nodes
  print usefully.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `helpers.ts:32` (the culprit filter) against the assertion
above it.

</details>

### ⚪ LOW · Build outputs are tracked, and one of them feeds the Nix package source

**`.gitignore:1`** · _quality_

`git ls-files` shows `result` (a symlink into a store path that exists only on
the author's machine) and both `test-results/.last-run.json` files as tracked;
the root `.gitignore` covers only `/target`, `.env` and `evals/fixtures/private/`,
and `web/.gitignore` lacks `test-results/`. Under jj every untracked file is
snapshotted into the next commit, so runs dirty the working copy and can sweep in
failure screenshots and traces. `flake.nix`'s `src = self` includes `result`, so
every `nix build` rewrites the package source and invalidates the Rust build; a
fresh clone gets a dangling symlink.

- **Suggested fix:** add `/result`, `/result-*`, `test-results/`,
  `playwright-report/` and `blob-report/` to the relevant `.gitignore`s and
  untrack the committed artifacts (`jj file untrack`).

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `git ls-files` output; `.gitignore` and `web/.gitignore`
contents; `flake.nix`'s `src = self`.

</details>

---

## Packaging (Nix) & evals

**Files:** `nix/module.nix` (the `services.mise` NixOS module), `flake.nix`,
`evals/src/main.rs`
**Read first:** implementation doc → *Server defaults / packaging*, *Auth*
(agenix, `LoadCredential`), *Evals*
**Key entry points:** the module's `serviceConfig`, `flake.nix`'s package
outputs, `evals::run_list`
**Theme:** the unit is called hardened and is *partly* hardened; the gaps that
remain are the ones that matter for a service which shells out to git and
fetches arbitrary URLs. Separately, `root` is configurable in a way the sandbox
does not follow.

### 🟠 MEDIUM · The corpus and everything in it are world-readable

**`nix/module.nix:94`** · _security_

The unit sets no `UMask` and no `StateDirectoryMode`, so `StateDirectory` creates
`/var/lib/mise` at 0755 and the service inherits umask 022 — making the photos
directory 0755 and the exported markdown 0644. The corpus is the most private
data in the system: pantry and fridge contents, the cook log, full assistant
transcripts, and the kitchen shelf photos the repo deliberately keeps out of git.
Every other local user and unsandboxed service can read all of it, in pointed
contrast to the care taken with the client's 0600 `remote.json`.

- **Spec:** implementation doc, *Server defaults*: "hardened systemd unit"; *Auth* (client tokens 0600).
- **Suggested fix:** add `UMask = "0077"` and `StateDirectoryMode = "0700"`,
  keeping the corpus root inside the state directory.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `nix/module.nix:86-111` is the complete `serviceConfig`: no
`StateDirectoryMode`, no `UMask`. A repo-wide grep for
`set_permissions|PermissionsExt|from_mode|umask|UMask|StateDirectoryMode` over
`*.rs`/`*.nix`/`*.md` returns nothing outside `crates/cli/src/remote.rs`. Write
side: `store.rs:139-141` creates root, `photos/` and `export/` with plain
`create_dir_all` (0755 under umask 022) and `:835-841` writes export markdown
with `std::fs::write` (0644).

</details>

### 🟠 MEDIUM · `ReadWritePaths` hardcodes the default root

**`nix/module.nix:105`** · _bug_

`services.mise.root` defaults to `/var/lib/mise/cookbook` but is an arbitrary
string, while the sandbox grants write access only to `/var/lib/mise`
(`StateDirectory` plus a redundant `ReadWritePaths`). Under `ProtectSystem =
strict` everything else is read-only, and no assertion ties the two together —
so setting `root = "/srv/cookbook"` produces a runtime EROFS restart loop rather
than an eval error, and the operator sees a read-only-filesystem error on a
directory that is plainly writable outside the unit. The planned per-instance
attrset will hit the same thing.

- **Spec:** implementation doc, *Server defaults / packaging*: `root` is the documented corpus root option.
- **Suggested fix:** use `ReadWritePaths = [ cfg.root ]` (keeping
  `StateDirectory` for the default), or assert that `cfg.root` lies under
  `/var/lib/mise` when the default sandbox applies.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `root` is `lib.types.str` with no validation at `module.nix:24-28`,
passed through as `--root ${cfg.root}` at `:88`; `ProtectSystem = "strict"` at
`:103`, `StateDirectory = "mise"` at `:94`, hardcoded `ReadWritePaths = [
"/var/lib/mise" ]` at `:105`. There are no `assertions` anywhere under `nix/`.
Runtime failure path: `server/src/main.rs:101-107` → `Store::create` →
`create_dir_all` at `store.rs:139-141`.

</details>

### ⚪ LOW · The "hardened" unit omits the sandbox options that matter most here

**`nix/module.nix:101`** · _security_

Present: `NoNewPrivileges`, `PrivateTmp`, `ProtectSystem=strict`, `ProtectHome`,
an empty `CapabilityBoundingSet`, `RestrictSUIDSGID`, `ProtectKernelTunables`,
`ProtectControlGroups`, `LockPersonality`. Missing, and all compatible with a
Rust server that execs git: `SystemCallFilter=@system-service` +
`SystemCallArchitectures=native`, `RestrictNamespaces`, `PrivateDevices`,
`ProtectKernelModules`, `ProtectKernelLogs`, `ProtectClock`, `ProtectHostname`,
`RestrictRealtime`, `ProtectProc=invisible`, `RemoveIPC`,
`RestrictAddressFamilies`. This matters more than usual because the process runs
an LLM tool loop with a `fetch_url` tool whose own guard is documented as not a
bulletproof SSRF boundary — the unit is the second line of defence.

- **Spec:** implementation doc, *Server defaults / packaging*: "hardened systemd unit."
- **Suggested fix:** add the listed directives and verify with `systemd-analyze
  security mise.service` that the git subprocess still runs.

<details><summary>Verification trail — code pointers</summary>

Confirmed. `nix/module.nix:86-111` — the present set and the absent set are both
as listed.

</details>

### ⚪ LOW · The default model is duplicated in the module and always passed

**`nix/module.nix:60`** · _spec-drift_

`client.rs` defines `DEFAULT_MODEL` and the server's `main.rs` uses it as the
clap default, so omitting `--model` yields the binary's default. The module
hardcodes the same literal and unconditionally appends `--model ${cfg.model}` to
`ExecStart`, so the binary default can never take effect on a NixOS host. The
constants agree today; the next bump will silently not reach any deployed
server.

- **Spec:** implementation doc, *Anthropic client*: "Default model `claude-opus-5`, overridable (`mise chat --model`, `services.mise.model`)."
- **Suggested fix:** make the option `types.nullOr types.str` defaulting to
  `null` and append `--model` only when set, so the binary's default stays
  authoritative.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `nix/module.nix:60` (the duplicated literal) and the
unconditional `ExecStart` append; `client.rs`'s `DEFAULT_MODEL`.

</details>

### ⚪ LOW · The flake's CLI has no runtime git

**`flake.nix:16`** · _quality_

`Store::export` unconditionally shells out to `git`. `flake.nix` lists git in
`nativeCheckInputs` and `nix/module.nix` puts it on the service path, but nothing
covers the `mise` CLI binary the same package installs — precisely the artifact
users get from `nix profile install` / `nix run`. The docs acknowledge the server
side; the client side has no guarantee.

- **Spec:** implementation doc: the export "shells out to system git (guaranteed present via the NixOS module)."
- **Suggested fix:** `wrapProgram` the installed binaries with `${pkgs.git}/bin`
  on PATH via `makeWrapper`, which also makes the module's path entry redundant
  rather than load-bearing.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `flake.nix:16` (`nativeCheckInputs`); `nix/module.nix`'s service
path; `store.rs`'s unconditional `git` invocations.

</details>

### ⚪ LOW · A misspelled eval scenario name exits 0 having run nothing

**`evals/src/main.rs:519`** · _quality_

`run_list` filters known scenario names against the requested ones, so
unrecognized arguments match nothing, the loop body never runs, the summary
prints "0 checks, 0 failed" and the process exits 0 — while the file header
designs the exit code as the number of failed checks precisely so a shell loop
notices regressions.

- **Spec:** implementation doc, *Evals*: scenarios score mechanical checks.
- **Suggested fix:** collect requested names matching no known scenario and bail
  listing the valid ones; alternatively exit non-zero when `run_list` is empty
  but arguments were given.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `evals/src/main.rs:519` (`run_list`'s filter) and the summary
that follows.

</details>

---

## Docs

**Files:** `docs/implementation.md`
**Read first:** `CLAUDE.md` → *Documents* (docs and code describe the same
system; disagreement is a bug in one of them)
**Theme:** one section predates a decision made later in the same document.

### ⚪ LOW · The page-model section still describes photos as corpus state

**`docs/implementation.md:390`** · _spec-drift_

Three places disagree. *The page model* (line 390) and the on-disk layout say
photos are a content-addressed blob directory referenced by hash from pages and
threads. The M6 decisions (lines 289-293) say photos are conversation input and
nothing binary enters the store, sync or the export. And the code still creates
`photos/` and declares `CREATE TABLE blobs`, both dead — a repo-wide grep finds
only the schema string and the `mkdir` — while `store.rs`'s module doc still
advertises "blob metadata". An auditor checking "no state that exists only in
SQLite" has to reason about a `blobs` table the export parser does not cover, and
only learns by exhaustive grep that it is dead.

- **Spec:** `CLAUDE.md`, *Documents*: docs and code describe the same system; disagreement is a bug in one of them.
- **Suggested fix:** update *The page model* and the on-disk layout to match M6,
  drop or explicitly reserve the `blobs` table and the `photos/` mkdir, fix the
  `store.rs` module doc, and bump the doc's last-updated tag.

<details><summary>Verification trail — code pointers</summary>

Non-bug finding. `docs/implementation.md:390` and `:289-293`; the dead schema
string and `mkdir` in `crates/store/src/store.rs`.

</details>
