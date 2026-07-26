# Design: a living cookbook & meal planner

*Working title: **Mise** (as in mise en place — placeholder, rename freely).*
*Last updated: 2026-07-16. This document covers UX and features, not implementation.*

## Vision

A personal cooking companion that behaves like a living document rather than a
chat log or a static recipe site. Inspirations: the Young Lady's Illustrated
Primer (a book that knows you, teaches you, and grows with you) and Google Wave
(documents and conversations are the same object, picked up and continued
indefinitely).

Today, a well-prompted chatbot produces good recipes but has two failure modes:

1. **Amnesia.** Every conversation starts fresh, so it converges on the same
   suggestions ("you like curry" → curry, forever). It doesn't know what was
   cooked last week, what worked, or what lesson was learned when the sauce split.
2. **Stale world-model.** The ingredient list in the prompt is a snapshot.
   Reality drifts: sundries run out, the butcher had duck legs on discount,
   you're standing in the shop looking at the actual shelf.

Mise fixes both by giving the assistant a persistent, structured world — a
cookbook, a pantry, a cooking log, a plan — that it reads and maintains across
every interaction, the way a coding agent works inside a codebase.

## Design principles

- **Everything is a page.** Recipes, the pantry, the queue, techniques, the
  log — all are plain, human-readable documents in a directory tree. The
  assistant is the primary editor, but hand-editing is always possible and
  never breaks anything. (An index/cache may exist beside the files, but the
  files are the truth.)
- **Conversations are attached, not ephemeral.** Every page carries its own
  persistent thread. A question asked about tonkotsu in March is still there in
  July, and the conversation resumes with full context.
- **The assistant edits like a wiki editor, not a chatbot.** When you say "I'm
  out of miso" or debrief a cook, it updates the relevant pages directly. Pages
  show what changed recently; changes are revertible. Trust with an audit trail.
- **Graceful decay.** The system must stay useful when neglected. Inventory
  tracks presence and rough freshness, never gram counts. A month of no updates
  should mean slightly stale suggestions, not a broken database demanding
  reconciliation.
- **Two postures, one world.** A desktop posture for planning and browsing, and
  a phone posture for the store. Same underlying pages, different lens.

## Core objects

### The Queue (home page)

The centerpiece. A loose, ordered list of "things to make soon" — deliberately
**not** a day-by-day grid. You cook in 2–3 day batches and plan one to two
weeks out; dates would be a lie.

Each queue entry shows:

- The dish (linked to its recipe page, or a stub if it's still just an idea).
- **Readiness**: can this be made right now, or does it need a shop trip — and
  which tier (walkable shop / butcher / town)?
- Rough effort class (weekday ≤1h vs. weekend project), so a glance answers
  "what can I make *tonight*?"
- Why it's here, when the assistant suggested it ("rotating away from curry",
  "uses the wakame", "next step on yeast").

Below or beside the queue, the **fridge state**: what's currently cooked and
being eaten through ("Sunday's mapo, ~1 serving left"), so coverage gaps are
visible ("you run out of food Thursday").

Queue entries are added three ways: you ask for suggestions, the assistant
proactively proposes (see Steering), or you drop in raw ideas ("something with
duck sometime").

### The Cookbook

The growing collection of recipes actually made and refined — the repertoire.
Each recipe is a living page:

- The recipe itself: ingredients (grams and teaspoons, metric), steps written
  for this kitchen (the wok, the combo microwave, the stand mixer — no
  hedging for equipment that isn't owned).
- **History strip**: dates made, and one-line outcomes distilled from debriefs.
- **The recipe evolves.** Post-cook lessons get folded into the page itself —
  next time you open it, the tamari substitution and the "reduce longer" note
  are simply part of the recipe, with the change history available if you want
  to see what the original said.
- Links: techniques used, plausible variations, related recipes.
- Its own persistent thread, for deep dives: scaling questions, "could this
  work with tofu", "why did step 4 fail".

Recipes enter the cookbook when a queue idea gets fleshed out, or when a cook
happens — not by bulk import. The cookbook is *what I make*, not *all recipes
that exist*. One-off experiments that flopped stay in the log but don't clutter
the cookbook.

### The Pantry

One inventory, where each item carries:

- **Presence**: have / running low / out.
- **Freshness** for perishables: rough purchase date ("chicken thighs, bought
  Tue"), not expiry bookkeeping.
- **Source tier**: where it comes from when it's gone —
  1. *Staples* — always restocked on sight (the current project.txt pantry list).
  2. *Walkable shop* — reliably available locally.
  3. *Butcher* — the walkable butcher.
  4. *Town* — needs the bus; batch these up.
- Free-form notes where useful ("the shop's version is bland; town one is better").

The pantry page also encodes the standing facts from project.txt: equipment,
preferences, spice tolerance, time budgets. These are pages too — editable,
conversational ("actually I bought an immersion blender").

**Update paths, in order of expected frequency:**

1. **In passing, via any conversation.** "Out of miso, picked up duck legs" —
   said to the planning assistant, a recipe thread, anywhere. The pantry
   updates as a side effect.
2. **Photos.** Snap the fridge, the pantry shelf, or a store shelf; the
   assistant reconciles what it sees against the list and shows the diff.
3. **Cook-time inference.** Marking a recipe as made prompts a light-touch
   "used up the coconut milk?" as part of the debrief, rather than silent
   auto-deduction.
4. **Hand edits.** Always possible, rarely expected.

### The Log

Append-only record of cooks: date, dish, servings produced, distilled debrief
verdict. This feeds the steering engine (variety, skill progression) and the
fridge state. Mostly machine-maintained; readable as a diary of what you've
been eating.

### The Technique wiki

Standalone living pages for techniques — velveting, tare, wok hei, sourdough
shaping, pan sauces. They grow organically: when a recipe thread digs into
*why* something works, the assistant distills the reusable part into the
technique page and links it. Recipes reference techniques instead of
re-explaining them, and the skill-building steering (below) uses these pages to
know what's been learned versus merely encountered.

## The conversation model

Two layers, one shared world:

### The planning assistant (global)

The main conversational surface, always available from the queue. It has tools
to **explore the corpus the way a coding agent explores a codebase**: search
recipes, read logs and pantry state, follow links, and edit any page. You never
tell it context it can already look up.

Typical asks:

- "Plan the next week or so" → it checks the log (what's recent), the pantry
  (what's aging, what's stocked), the steering goals, and proposes 3–4 cook
  events with reasoning. Accepted ones land on the queue.
- "What can I make with this?" (photo or description, often in-store).
- "I've got people coming Saturday" → scaling, ambition level, shopping needs.

### Page threads (local)

Every page has its own thread that never expires. The recipe thread is where
you ask "can I halve the sugar", report mid-cook problems, and do the post-cook
debrief. The technique thread is where "explain gluten development like I'm
five" lives. Context is the page plus its history — focused, resumable,
indefinite.

The two layers share everything: a fact learned in a recipe thread ("hates
cleaning the meat grinder") is world-knowledge, not thread-local.

## Key flows

### Planning session (desktop)

Open the queue. Fridge state shows coverage ending Wednesday. Ask the assistant
to plan; it proposes cook events with readiness annotations. Accept/modify via
conversation. Byproduct: the shopping list updates, split by source tier.

### Store mode (phone)

Opening on the phone leads with:

- **The tiered shopping list** for wherever you are — checkoff as you go, items
  grouped by shop/butcher/town. Checking items updates the pantry.
- **Photo recon**: snap a shelf or the discount bin. The assistant answers the
  question you actually ask, which is "what can I make with this?" — pivoting
  the queue around opportunities ("duck legs discounted → duck curry... no
  wait, you've had curry twice this month. Braised duck with...").
- Quick "in passing" updates: "they're out of spring onions."

Store mode is fast and terse: big touch targets, minimal reading, answers
first, reasoning on tap.

### Cooking a recipe

Open the recipe page (any device). The page is the current, evolved version —
no re-deriving past lessons. Mid-cook questions go to the recipe thread
("it's not thickening — heat up or slurry?").

### Post-cook debrief

After cooking, the recipe thread asks how it went. A short conversation —
what was substituted, what surprised, verdict. The assistant then:

1. Folds durable lessons into the recipe page (visible as a recent change).
2. Appends the log entry (date, servings, one-line verdict).
3. Updates fridge state (leftovers) and touches pantry freshness/presence.

The debrief is skippable; a bare "made it, fine" still logs and updates state.

## Steering (the anti-curry engine)

The assistant actively steers rather than waiting to be asked, using the log
and the technique wiki. Steering shows up as *reasoned suggestions on the
queue*, never as blocking rules.

Priority order:

1. **Cuisine & dish rotation.** Track recency across cuisine, protein, and
   format (soup/stir-fry/bake/braise...). The prompt-bias toward favorites is
   counterweighted by "you've had this axis three times running."
2. **Skill-building.** A lightweight, visible agenda derived from stated goals
   (currently: yeast beyond sourdough, weekday-dinner fundamentals). The
   assistant occasionally picks a recipe *because* it teaches something, says
   so, and tracks progression on the technique pages. This is Primer-mode:
   a curriculum that emerges from cooking, not homework.
3. **Use-it-up pressure** (mild). Aging perishables and open jars nudge
   suggestions but never dominate them.

Explicitly *not* a steering input: seasonality as a concept. What the shops
stock already reflects the season and shows up through the pantry and store
mode.

## Nutrition awareness

Commentary, not logging. The assistant notices patterns across the queue and
recent log — "this week is fried-heavy", "almost no vegetables since Sunday" —
and mentions them while planning. No calorie counts, no daily targets, no
guilt UI.

## Editing & trust model

- The assistant edits pages directly as a side effect of conversation.
- Every page shows **recent changes** (what, when, from which conversation),
  and any change can be reverted.
- Full history is kept; "what did this recipe say before March?" is answerable.
- Hand edits and assistant edits are the same kind of thing in the same files.

## Out of scope (v1, deliberately)

- **Day-by-day meal grid.** The queue + fridge coverage replaces it.
- **Quantity tracking.** No "~200g tofu left"; presence + freshness only.
- **Sourdough/starter scheduling.** The starter lives in the fridge and in
  your head; the system may *know* a bake takes lead time, but doesn't manage
  the feeding rhythm.
- **Standalone rating system.** Verdicts live in debriefs and the log, not a
  star widget.
- **Bulk recipe import / web clipper.** The cookbook grows by cooking.
- **Multi-user support.** Cooking for one (guests are a conversation, not a
  feature).
- **Nutrition logging.** Awareness only, as above.
- **Silent inventory auto-deduction.** Updates go through the debrief touch.

## Open questions

- **Naming & structure of the corpus**: what does the directory tree actually
  look like (recipes/, techniques/, pantry.md, queue.md, log.md, threads
  alongside or inside pages?). Decide at implementation time; constraint is
  human-readability and hand-editability.
- **Phone delivery**: PWA vs. something else — implementation question, but
  store mode's photo capture and offline tolerance (shop basements have bad
  signal) should drive it.
- **Thread hygiene**: threads never expire, but do they need summarization to
  stay fast/cheap as they grow? Probably yes, invisibly.
- **How proactive is proactive**: does the assistant ever reach out (a morning
  "you're out of food tomorrow" nudge), or only speak when the app is open?
  Leaning: no push notifications in v1; the queue's coverage warning is enough.
