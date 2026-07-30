# Evals

Prompt- and judgment-quality checks for the assistant, run **manually**
against the real Anthropic API. Never part of the test suite, never in CI —
the test suite proves the deterministic machinery; these measure whether the
assistant is any *good*.

```sh
ANTHROPIC_API_KEY=… cargo run -p mise-evals            # all scenarios
cargo run -p mise-evals -- plan-week pantry-in-passing  # a subset
```

(A git-ignored `.env` with the key works too.)

Each scenario seeds a fresh corpus, runs one or more real exchanges, then
prints the transcript plus a checklist: the mechanical parts (did it look
before proposing? did the queue/pantry/log actually change?) are scored
automatically; tone and judgment are yours to read.

Scenarios: `plan-week`, `pantry-in-passing`, `debrief`, `draft-from-url`,
`calculator-page`, `pantry-recon`. The URL scenarios script the network
with fixture pages: `tonkatsu.html` (life-story-heavy, no JSON-LD — the
draft must keep the substance, record the source URL, and leave the
narration on the blog) and `pancake-calculator.html` (quantities computed
client-side and absent from the fetch — the assistant must ask instead
of inventing numbers).

`pantry-recon` reads real shelf photos from `fixtures/private/`
(gitignored — shelf photos are personal data, like the corpus; the
scenario skips itself when the directory is empty). Drop in a couple of
`.jpg`/`.png`/`.webp` shots — a full shelf and a sparse one are the
useful pair. Mechanical checks only prove the posture (proposed, didn't
edit; the photo touched nothing); whether the model actually read the
shelf right is yours to judge, printed proposal against photo.
