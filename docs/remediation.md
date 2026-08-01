# Remediation campaign — audit findings before M7

*Last updated: 2026-08-01.*

Working sequence for the 94 findings in
[the 2026-07-31 review](reviews/2026-07-31-codebase-review.md). That document
is the registry — what each finding is, why it is wrong, and its verification
trail. This one is only the **order of work**, so the campaign survives across
sessions. Findings are referenced by id; they are not restated here.

Five findings are deliberately out of scope and scheduled after M7 (recorded in
[implementation.md](implementation.md) → *Known, scheduled after M7*): #2, #45,
#49, #51, #52. **89 in the campaign.**

## Rules for this campaign

- **The atom is `[regression test + fix]` in one commit.** `CLAUDE.md` wants the
  test first; jj wants each committed chunk green; a red test cannot be
  committed. Where the fix makes the bug class unrepresentable, the commit
  message says so and skips the test.
- **Doc edits ride with the behaviour change** they describe, per `CLAUDE.md`.
  The only docs-only commit is Phase 1, which is a constraint on everything
  after it.
- **Group by code path, not severity.** Several findings share one fix.

## Ordering hazards

| | Hazard |
|---|---|
| H1 | #9, #20, #21 all want one inner `append_changes(&Transaction)`. Design that API once, then `create_doc` and `Peer::commit` both use it. |
| H2 | #17 and #18 interact — normalizing on insert decides what the uid hashes. One shared normalize-then-hash row constructor; sync verifies by reconstruction. Collapses #17, #18, #19, #16. |
| H3 | `Thread.svelte` is six findings in forty lines. One designed target state, in the Phase 5 order, or `send()` gets rewritten six times. |
| H4 | The auth layer deletes `authorized()` from ten handler bodies. It lands **before** #8 and the `/chat` body-limit fix, which touch those same bodies. |
| H5 | Export self-heal (#13) lands **before** any error-path export (#34, #59), or the number of places that fail after the store already committed multiplies. |
| H6 | The round transaction does not subsume #22 — `sync.rs:8-13` documents that a queue doc may legitimately be ahead of its recipe docs. |
| H7 | No shape change (#14, #15) lands before the Phase 2 hydrator and the #7 destructure exist. This is the only unrecoverable hazard. |

## Phases

### Phase 0 — Repo hygiene
- [x] Build outputs stay out of the tree — #83

### Phase 1 — Docs & schema policy *(docs only)*
- [x] Schema-change policy; honest fetch guarantee; photos drift; uid decision; defrost settled — #92, and the doc halves of #19, #48, #94, #5

### Phase 2 — Guardrails & hydrate mechanism
- [x] Shared server test harness *(enabler — four `spawn_*` copies today)*
- [x] Revert destructures its docs — #7, #10 *(must precede H7)*
- [x] Tolerant hydrate mechanism + historical doc-byte fixtures *(policy)*
- [x] Sync wire carries a schema version — #24
- [x] Shutdown drains; the corpus is private — #84, #86, #87

### Phase 3 — Data integrity: store & sync
- [x] One transaction per doc creation and per sync round — #9, #21 *(H1)*
- [x] Snapshots are rebuilt, not copied from a stale session doc — #20
- [x] A one-time snapshot repair on open *(recovers what #20 hid — the changes are still in `doc_changes`)*
- [x] Sync verifies uids and normalizes on ingest; uids go replica-scoped — #17, #18, #19, #16 *(H2)*
- [x] The export regenerates itself — #13 *(H5)*
- [x] An incomplete location degrades instead of erasing the export — #22 *(H6)*
- [ ] WAL and a busy timeout — #12
- [ ] Shopping items and fridge portions get replica-safe ids — #36, #93 *(legacy `s1` keys go inert, never reused)*
- [ ] Coverage saturates; servings are bounded at ingress — #0, #1
- [ ] Interrupted sessions and hostile peers are tested — #23
- [ ] Recipe status is an enum; equipment and pantry links are slugs — #14, #15 *(first use of the hydrator)*

### Phase 4 — Trust boundary
- [x] Rendered markdown is sanitized — #67 *(jumped the queue: no dependencies, highest severity)*
- [x] v4-mapped literals are refused — #46, #50
- [ ] A CSP on static responses *(split from #67; needs an e2e run to prove the SPA still boots)*
- [ ] Auth is a layer, not ten call sites — #56, #57, #77 *(H4; route-table test first)*
- [x] The token file's mode is enforced on every write — #61
- [ ] Provenance is normalized before it enters history — #41

### Phase 5 — Store-mode readiness (web + tools)
- [ ] 401 loops back to the gate from every call site — #73 *(five lines, closes ~13 sites)*
- [ ] A shared composer, used by the thread and the drafting box — #72
- [ ] An exchange can be cancelled, and a failed send keeps your work — #68, #69, #78
- [ ] Photos are downscaled one at a time and capped before upload — #74, #75
- [ ] One frame budget, honestly enforced — #53, #54, #55
- [ ] Tool inputs reject what they don't understand — #37, #39
- [ ] Edit affordances only appear where editing works — #70, #76
- [ ] The queue survives a dangling recipe reference — #40 *(after #4)*

### Phase 6 — Model path & assistant loop
- [ ] Byte boundaries survive chunk splits — #25, #28, #81
- [ ] A truncated turn reports truncation — #26, #32, #33
- [ ] The model client has deadlines and retries — #27
- [ ] Photos attach to the message they belong to — #31, #91
- [ ] An aborted exchange leaves the export honest — #34 *(after #13)*
- [ ] Extraction runs under a deadline, off the runtime — #47

### Phase 7 — Edges & operations
- [ ] The CLI renders the queue from `views`, not a copy — #4
- [ ] Sync always exports what it persisted — #58, #59
- [ ] Remote URLs and join failures are recoverable — #60, #63
- [ ] Remove reports what actually happened — #64
- [ ] Sync reports what actually happened — #62, #66
- [ ] Remote mode's guarantees are tested — #65
- [ ] A revert hash can't panic the handler — #8 *(after H4)*
- [ ] The unit follows `services.mise.root` — #85, #89, #90, #88

### Phase 8 — Remaining drift & quality
- [ ] Shop needs are one per pantry item — #3
- [ ] Context assembles only what the prompt needs — #35, #29, #30
- [ ] The assistant can see recipe status — #38
- [ ] Edits change only the fields they name — #42, #43
- [ ] Provenance and lead time are asserted, not assumed — #44, #6
- [ ] The e2e suite can actually fail — #79, #80, #82

## Done when

- `cargo test` green; `cd web && npm run e2e` green at 375px.
- Historical doc fixtures hydrate, and `revert` reaches each of them.
- Two real stores through `run_sync` (basement checkoff) converge **and**
  export byte-identically.
- `rm -rf export/`, run any mutation: it regenerates and commits.
- A thread-only sync produces `export/threads/planning.md` on the receiver.
- `cargo run -p mise-evals` by hand — recon and draft-from-URL unchanged.
