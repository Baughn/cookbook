# Design: a living cookbook & meal planner

*Working title: **Mise** (as in mise en place — placeholder, rename freely).*
*Last updated: 2026-07-30. This document covers UX and features, not implementation.*

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
  log — all are human-readable pages. The assistant is the primary editor;
  direct editing is always available in the app. The store is the truth, and
  beside it lives a deterministic, read-only **markdown export** of the whole
  corpus — a plain directory tree that is greppable, diffable, readable in
  any editor, and a complete exit strategy that outlives the software.
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
  which tier (walkable shop / butcher / town)? Readiness is computed against
  the **active location** (see Locations): its pantry, its shops, *and its
  equipment* — a wok recipe isn't ready in a kitchen without one. Readiness
  also knows about **lead time**: a marinade, a defrost, a levain make a dish
  "ready tomorrow if you start tonight", and the queue surfaces the act-now
  step instead of silently calling the dish makeable.
- Rough effort class (weekday ≤1h vs. weekend project), so a glance answers
  "what can I make *tonight*?"
- Why it's here, when the assistant suggested it ("rotating away from curry",
  "uses the wakame", "next step on yeast").

An entry is usually a single dish, but can be a small **menu** — the
Saturday-guests entry bundles a main and sides that shop, scale, and cook
together.

Below or beside the queue, the **fridge state**: what's currently cooked and
being eaten through ("Sunday's mapo, ~1 serving left"), so coverage gaps are
visible ("you run out of food Thursday"). The **freezer** — freezers, once
the chest freezer arrives; a location can have several — is tracked
separately: frozen portions are long-tail coverage ("you run out Thursday —
unless you defrost the March bolognese"), and frozen raw proteins add a
defrost step to readiness. Same presence-and-rough-date model, much slower
decay. Coverage warnings only speak when the app is open; there is no
away-mode to set — not opening it *is* the away mode.

Queue entries are added three ways: you ask for suggestions, the assistant
proactively proposes (see Steering), or you drop in raw ideas ("something with
duck sometime").

Entries age visibly. The assistant prunes conversationally ("still want
this?"), and repeated deferral is itself a steering signal ("you keep
skipping the soufflé — too ambitious for weekdays?"). Below the active queue
sits a **someday shelf** — "would like to try this someday" — so the queue
proper stays an honest short list of intent, not a graveyard of aspirations.

The queue itself is **global, not per-location**: it holds desires, and
desires travel with you. Being at the cottage is a lens ("what's makeable
*here*?"), not a separate queue — per-location queues would fragment intent
and leave the cottage one stale for months.

### The Cookbook

The growing collection of recipes actually made and refined — the repertoire.
Each recipe is a living page:

- The recipe itself: ingredients (grams and teaspoons, metric), a **servings
  count** (the base for scaling and coverage math), steps written
  for the primary kitchen (the wok, the combo microwave, the stand mixer — no
  hedging for equipment that isn't owned). Recipes do **not** get per-location
  variants; when cooking elsewhere, the assistant knows the active location's
  equipment page and adapts or flags at queue/cook time.
- **History strip**: dates made, one-line outcomes distilled from debriefs,
  and any debrief photos — the crumb shot, the finished dish. The cookbook
  should look like *your* cooking, and "did it look like this last time?" is
  a real question.
- **The recipe evolves.** Post-cook lessons get folded into the page itself —
  next time you open it, the tamari substitution and the "reduce longer" note
  are simply part of the recipe, with the change history available if you want
  to see what the original said.
- Links: techniques used, plausible variations, related recipes — and,
  when the recipe was drafted from somewhere, its source URL. A page
  that used a source says so.
- Its own persistent thread, for deep dives: scaling questions, "could this
  work with tofu", "why did step 4 fail".

Recipes enter the cookbook when a queue idea gets fleshed out, when a cook
happens, or as a **draft** — out of curiosity, or from a URL you deliberately
handed the assistant — never by bulk import. The cookbook is *what I make*,
not *all recipes that exist*: drafts sit on their own shelf, out of steering's
rotation, until a first cook promotes them. One-off experiments that flopped
stay in the log but don't clutter the cookbook.

Repertoire also shrinks. A recipe you've gone off can be **retired**: out of
steering's rotation and the browse surface, but never deleted — the history
and lessons keep their value, and un-retiring is one edit. Draft, active,
retired: one status field, and moving between them is one edit.

The cookbook is human-browsable, not only assistant-mediated: by cuisine,
protein, format, effort class — conveniently the same metadata the steering
engine already tracks.

### The Pantry

One inventory **per location** (see Locations), where each item carries:

- **Presence**: have / running low / out.
- **Freshness** for perishables: rough purchase date ("chicken thighs, bought
  Tue"), not expiry bookkeeping.
- **Source tier**: where it comes from when it's gone. Tiers are defined by
  the location; home's are —
  1. *Staples* — always restocked on sight (the current project.txt pantry list).
  2. *Walkable shop* — reliably available locally.
  3. *Butcher* — the walkable butcher.
  4. *Town* — needs the bus; batch these up.
- Free-form notes where useful ("the shop's version is bland; town one is better").

The standing facts from project.txt split along the person/place line:
**equipment** lives on the location; **preferences, spice tolerance, time
budgets** are global (see Standing facts). These are pages too — editable,
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
4. **Direct edits in the UI.** Always possible, rarely expected.

### The Shopping List

A first-class page, not just a byproduct of planning. It aggregates what the
queue needs, staples spotted running low (tier 1 is *restock on sight*), and
free-form additions ("dish soap", "more of the good soy sauce"), grouped by
the active location's source tiers. Unchecked items carry over to the next
trip; the assistant prunes stale ones conversationally rather than letting
the list rot.

Every entry carries two one-tap actions:

- **Bought** — checks it off and updates the pantry as a side effect.
- **Unfindable** — opens a short, structured exchange. The assistant answers
  with a near-strict enumeration of concrete options — a substitute from the
  shelf in front of you, an alternate dish for the queue entry that needed
  it, defer to a higher tier — plus a free-form escape hatch. Choices ripple
  immediately: pick the substitute and tonight's readiness recomputes.

Like every page it has its own thread — "which of these soy sauces is
closest to tamari?" belongs to the list, mid-aisle.

### Locations

You cook in more than one kitchen: home, and (currently) the family cottage in
Norway. Each is a **location** — a small bundle of everything that belongs to a
*place* rather than to *you*:

- Its own **pantry** (inventory, presence, freshness).
- Its own **equipment** page — the wok and stand mixer are at home; the
  cottage has whatever it has.
- Its own **source tier definitions**. "Walkable shop / butcher / town"
  describes home's geography; the cottage might have just "the local shop" and
  "the drive to town", with a very different reliably-available set. Tiers are
  not a fixed enum.
- Its own **fridge & freezer state** (a location can have several freezers).
  Sunday's mapo at home does not feed you at the cottage; coverage warnings
  are computed against where you are.
- Standing facts about the place, e.g. "usually cooking for 2 here."

Everything else — cookbook, log, techniques, steering goals, preferences,
spice tolerance — belongs to the person and stays global, viewed through the
lens of the **active location**.

The active location is a manual, sticky selector, always visible: a silently
wrong location means "out of miso" quietly corrupts the wrong pantry, which is
a trust-model failure. The assistant may switch it conversationally ("we're at
the cottage") with confirmation, but never by geolocation guesswork.

Secondary locations decay hard — the cottage pantry is neglected ~90% of the
year, and unlike home, *other people* change it while you're away. Its state
carries lower confidence (the assistant verifies more before relying on it),
and the photo-recon flow doubles as an **arrival ritual**: snap the cottage
shelves when you arrive, reconcile, carry on. The presence-and-rough-freshness
model is what makes this survivable; a stricter inventory would be
unrecoverable after five months away.

### The Log

Append-only record of cooks: date, dish, **location**, servings produced,
distilled debrief verdict. This feeds the steering engine (variety, skill
progression) and the fridge state. Rotation is about the person, not the
kitchen — curry at the cottage still counts toward curry recency. Mostly
machine-maintained; readable as a diary of what you've been eating.

Not every cook is a meal. Cook events are typed — **meal**, **bake**, or
**staple production** (tare, stock, chilli oil, pickles). Staple production
feeds the pantry rather than the fridge state: the batch of chilli oil
becomes an inventory item with a freshness date, and recipes can depend on
it. A loaf covers no dinners; the tare covers the next three ramen nights.

### The Technique wiki

Standalone living pages for techniques — velveting, tare, wok hei, sourdough
shaping, pan sauces. They grow organically: when a recipe thread digs into
*why* something works, the assistant distills the reusable part into the
technique page and links it. Recipes reference techniques instead of
re-explaining them, and the skill-building steering (below) uses these pages to
know what's been learned versus merely encountered.

### Standing facts (memory)

The global home for what the assistant knows about *you and yours* that no
other page owns: preferences, spice tolerance, time budgets, quirks ("hates
cleaning the meat grinder"), and the people you cook for — "Anna is
vegetarian, Dad hates coriander" is exactly the kind of fact that should
never be asked twice. Structurally this is a memory subsystem: many small
facts, written as a side effect of any conversation, consulted while
planning. Guests remain a conversation, not a user account — the
conversation just has memory. Same trust model as everything else: visible,
editable, revertible.

## The conversation model

Two layers, one shared world — plus one standing side table:

### The drafting table

Where recipes are negotiated into existence: the cookbook's new-recipe
box. A URL or a description goes in; a draft page comes out — or a
question, when the substance needs the cook's input first. Its own
persistent thread (`threads/drafting`), so planning stays planning and a
half-finished negotiation survives a page reload; once the draft exists,
its page thread takes over.

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

### First run (bootstrapping)

Day one is the cottage-arrival ritual pointed at home: seed the location's
equipment and staples from a conversation (or the old project notes), then
photo-recon the shelves and fridge to reconcile guesses against reality. An
empty log means steering has nothing to push against — early on the
assistant asks rather than infers ("what have you been eating lately?"), and
the queue starts from stated cravings instead of rotation math.

### Planning session (desktop)

Open the queue. Fridge state shows coverage ending Wednesday. Ask the assistant
to plan; it proposes cook events with readiness annotations. Accept/modify via
conversation. Byproduct: the shopping list updates, split by source tier.

### Store mode (phone)

Opening on the phone leads with:

- **The tiered shopping list** for wherever you are — items grouped by
  shop/butcher/town, each with its two taps: bought (updates the pantry) or
  unfindable (structured substitution exchange — see The Shopping List).
- **Photo recon**: snap a shelf or the discount bin. The assistant answers the
  question you actually ask, which is "what can I make with this?" — pivoting
  the queue around opportunities ("duck legs discounted → duck curry... no
  wait, you've had curry twice this month. Braised duck with...").
- Quick "in passing" updates: "they're out of spring onions."

Store mode is fast and terse: big touch targets, minimal reading, answers
first, reasoning on tap.

### Trip prep (changing location)

Before a cottage stay, tell the assistant you're going. It plans against the
*destination*: what the cottage pantry probably still has (at low confidence),
what its shops can supply, and — the genuinely new artifact — a **"bring from
home" packing list**: the wakame, miso, and Sichuan peppercorns that no local
tier can provide. On arrival, the photo-recon ritual (see Locations)
reconciles reality; the selector flips and every readiness annotation,
coverage warning, and shopping list now speaks for the cottage.

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
   suggestions but never dominate them. This is the one steering input scoped
   to the active location — the open jar at home is irrelevant until you're
   back; rotation and skill-building follow the person everywhere.

The rotation goals and skill agenda live on their own **steering page** —
"stop pushing yeast for a while" is an edit (or a sentence to the
assistant), not a plea against a black box.

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
- Your edits and assistant edits are the same kind of thing in the same
  history.

## Out of scope (v1, deliberately)

- **Day-by-day meal grid.** The queue + fridge coverage replaces it.
- **Quantity tracking.** No "~200g tofu left"; presence + freshness only.
- **Sourdough/starter scheduling.** The starter lives in the fridge and in
  your head; the system may *know* a bake takes lead time, but doesn't manage
  the feeding rhythm.
- **Standalone rating system.** Verdicts live in debriefs and the log, not a
  star widget.
- **Bulk recipe import / web clipper.** The cookbook grows by cooking.
  Handing the assistant one URL you chose is a different thing — it drafts
  the page in house style, life story omitted; what's excluded is wholesale
  clipping.
- **Multi-user support.** Cooking for one (guests are a conversation, not a
  feature). A location may carry "usually cooking for 2 here" as a standing
  fact — that's a property of the place, not a second user account. Friends
  who want the app get their own corpus — forked or fresh, never shared
  state — so every corpus stays single-user.
- **Nutrition logging.** Awareness only, as above.
- **Silent inventory auto-deduction.** Updates go through the debrief touch.

## Open questions

- **Naming & structure of the corpus**: what does the directory tree actually
  look like (recipes/, techniques/, queue.md, log.md, threads alongside or
  inside pages? locations/home/ and locations/cottage/ each holding pantry,
  equipment, shops, fridge state, with one line of global state naming the
  active location?). Decide at implementation time; constraint is
  human-readability of the export.
- **Phone delivery**: PWA vs. something else — implementation question, but
  store mode's photo capture and offline tolerance (shop basements have bad
  signal) should drive it.
- **Thread hygiene**: threads never expire, but do they need summarization to
  stay fast/cheap as they grow? Probably yes, invisibly.
- **How proactive is proactive**: does the assistant ever reach out (a morning
  "you're out of food tomorrow" nudge), or only speak when the app is open?
  Leaning: no push notifications in v1; the queue's coverage warning is enough.
- **Sync & conflicts**: two postures plus offline store mode means concurrent
  edits will happen (checking off items in a shop basement while a desktop
  thread edits the pantry). Pages must merge at item granularity; leaning
  CRDT — there's no trust boundary here, the server can trust the client —
  but whole-page last-writer-wins is not good enough for the pantry. Decide
  at implementation time.
