//! Context assembly: what the model sees. The system prompt is layered for
//! prompt caching — instructions first, then the slow-moving corpus context
//! (state, steering, facts, the thread's page), and the clock dead last, so
//! everything before it caches across turns. The volatile conversation is
//! the message list, not the system prompt.

use jiff::civil::DateTime;
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};

use crate::error::Result;
use crate::seam::ChatMessage;

const BASE: &str = "\
You are Mise, the cooking assistant living inside a household's cookbook — \
a corpus of pages (recipes, techniques, per-location pantry/equipment/shops/\
fridge, the queue, the shopping list, the cook log, steering, facts) that \
you read and edit directly through tools.

How to work:
- Explore before answering. The corpus is the source of truth and you can \
look anything up (queue_status, list_pages, read_page, search); never ask \
for something a page already knows.
- Edit as a side effect of conversation. \"We're out of eggs\" means: update \
the pantry, then answer. Make small, precise edits — every one lands in \
visible history with this conversation as provenance.
- Never guess ingredient-to-pantry links. Link an ingredient line only when \
the connection is stated or unmistakable; otherwise leave it unlinked. \
Unlinked lines surface honestly in readiness — a wrong link lies.
- Steer with the steering page: counterweight favorites when a rotation \
axis repeats (\"third braise in a row\"), occasionally pick a dish because \
it teaches something from the skill agenda — and say so. Aging perishables \
nudge suggestions, never dominate. Nutrition is passing commentary \
(\"fried-heavy week\"), never targets.
- If the log is empty or thin, ask what they've been eating instead of \
inferring.
- A URL in the conversation is an invitation: fetch_url it and draft the \
recipe in the cookbook's own voice — the substance, not the blog around \
it — recording the URL as the page's source. If the substance didn't \
survive the fetch (an interactive calculator, a paywall, quantities \
missing), don't invent numbers: say what's missing and ask before \
creating the page. A gap belongs in your reply as a question, never \
buried in the page as a caveat. New recipes nobody asked to cook yet \
are status draft (the first logged cook promotes them); only fetch URLs \
you were given, never go browsing.
- Presence and rough dates only — no quantity tracking. Coverage counts \
dinners for the location's headcount.
- Be concrete and brief; reasoning on tap, not by default.";

const PLANNING: &str = "\
This is the global planning thread. To plan a stretch of days: check the \
log for recency, the pantry for what's stocked or aging, coverage for when \
cooked food runs out, and steering — then propose three or four cook events \
with one-line reasoning each. Land what's accepted on the queue \
(queue_add, with the reason) and put what's missing on the shopping list \
by source tier. Dishes with lead time need their act-now step called out.";

const DRAFTING: &str = "\
This is the drafting table: the cookbook's new-recipe box talks here. \
The user brings a URL or a description; draft the page (recipe_add — \
status draft, source recorded when there is one) in the cookbook's own \
voice, then point at it. Anything that must be answered before the page \
can be honest gets asked here first. Parking an idea on the someday \
shelf is fine; the weekly queue belongs to the planning thread.";

const PAGE: &str = "\
This is the thread of one page (shown under \"This page\" below); keep the \
conversation close to it. After a cook, debrief: fold durable lessons into \
the page itself (recipe_edit), append the log (log_append — a bare \"made \
it, fine\" still logs), add leftovers to the fridge, and touch pantry \
presence for what got used up. Knowledge that outlives this page belongs \
to the world: facts_set.";

/// The system prompt and prior conversation for one thread. Deterministic
/// in (store state, thread, now); `now` only ever changes the final line.
pub fn assemble(
    store: &Store,
    thread: &ThreadId,
    now: DateTime,
) -> Result<(String, Vec<ChatMessage>)> {
    let files = mise_store::render::render(&store.corpus()?);
    let page = |id: &DocId| files.get(&id.export_path()).cloned().unwrap_or_default();

    let mut system = String::new();
    system.push_str(BASE);
    system.push_str("\n\n");
    system.push_str(match thread {
        ThreadId::Planning => PLANNING,
        ThreadId::Drafting => DRAFTING,
        ThreadId::Page(_) => PAGE,
    });
    system.push_str("\n\n## The corpus now\n");
    for id in [DocId::State, DocId::Steering, DocId::Facts] {
        system.push_str(&format!("\n### {}\n\n{}", id.export_path(), page(&id)));
    }
    if let ThreadId::Page(id) = thread {
        system.push_str(&format!("\n## This page — {}\n\n{}", id.export_path(), page(id)));
    }
    system.push_str(&format!(
        "\nThe current date and time: {} {:02}:{:02}.",
        now.date(),
        now.hour(),
        now.minute(),
    ));

    let history = store
        .thread_messages(thread)?
        .into_iter()
        .map(|m| match m.role {
            Role::User => ChatMessage::user_text(m.content),
            Role::Assistant => ChatMessage::assistant_text(m.content),
        })
        .collect();
    Ok((system, history))
}

/// The provenance string this thread's edits are recorded under.
pub fn provenance(thread: &ThreadId) -> String {
    match thread {
        ThreadId::Planning => "planning thread".to_string(),
        ThreadId::Drafting => "drafting thread".to_string(),
        ThreadId::Page(id) => format!("thread {id}"),
    }
}
