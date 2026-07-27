# Testing

Tests follow a hierarchy:

1. Correct by construction.
   If an error state can be made unrepresentable, then it does not need to be
   tested. In particular, this means types and program structure should not be
   skewed to support the creation of tests which would not otherwise be
   necessary. Use common sense, but prefer code to be correct by construction.
2. Property tests and fuzz tests.
   Tests which check *many* states are preferable to edge-case tests which
   check just *one*. However, critical edge cases should still be tested
   explicitly.
3. Unit tests.

## Principles

- **Deterministic and reproducible**: Seeded RNG, paused tokio time, no flaky
  sleeps. Every test failure should be reproducible from the seed alone.
- **Spec-driven**: Write tests from the specification, not the implementation.
  If the spec is unclear, clarify it before writing the test; don't assume
  that either the spec or the code is correct, outside of obvious cases.
- **Regression tests first**: When fixing a bug, write a test that reproduces
  it *before* writing the fix. Exception: when the fix makes the bug class
  unrepresentable (in the types, or structurally), the test is optional —
  never skew a design to keep a bug representable for testing's sake;
  provably correct beats tested correct. A regression test still fits when
  it models the real triggering conditions through the honest interface
  without bending the design.
- **High-risk areas get extra coverage**: CRDT convergence, for instance.

## Project-specific

- **No model in the test suite.** All LLM interaction sits behind a seam;
  everything below it (readiness, coverage, rotation, lead time, page
  mutations) is deterministic logic tested with scripted inputs. If a piece of
  logic can't be tested without a model, it's on the wrong side of the seam.
  Prompt and judgment quality are measured by evals, kept separate from the
  test suite — never as pass/fail gates in CI.
- **The export never lies.** The markdown export is the readable backup and
  exit strategy; property test it at the store layer. Same doc state →
  byte-identical export, always; and the export is complete — everything in
  the store is legible somewhere in the export, with no state that exists
  only in SQLite. Completeness is verified by a **test-only parser**: export
  → parse → structural compare against store state for structured pages,
  frontmatter checks for prose. The parser lives in test code only and must
  never grow into an input path.
- **CRDT convergence, concretely.** Generate random operation sequences and
  apply them under seeded interleavings and partition/merge scenarios; assert
  identical final state. Idempotence and commutativity are standalone
  properties. The motivating scenario — offline shopping-list checkoffs in a
  signal-dead store while a desktop thread edits the pantry — gets an explicit
  named test. Converged replicas also produce byte-identical exports — the
  composed property (convergence ∘ export determinism) gets its own named
  test, since it is the user-facing promise: two devices, same files.
- **Domain invariants are properties.** Readiness is monotone — adding
  pantry items or equipment never makes a ready dish unready. Coverage is
  monotone in fridge servings. Lead-time readiness is consistent under time
  shift: ready at t with lead L ⟺ the act-now step is due at t−L. Test
  these as properties over generated states, not example tables.
- **Time is an input.** Freshness decay, queue aging, coverage horizons, and
  lead-time math take the clock as a parameter; no logic reads wall time. This
  applies to plain functions, not just async code under paused tokio time.

# Documents

Update this list whenever a document is added.

Overall design & goals: docs/design.md
Implementation: docs/implementation.md

Docs and code describe the same system, and the invariant is *agreement* — not
"every commit updates the docs." When a change alters behavior the docs
describe, the doc edit belongs in the same change. When a bugfix brings the
code back to what the docs already said, the fix *is* the sync; touching the
docs would be noise. When you find the two disagreeing, that's a bug in one of
them — figure out which is wrong before editing either, and ask the user on any uncertainty.
Bump a doc's "last updated" tag whenever you edit it.
