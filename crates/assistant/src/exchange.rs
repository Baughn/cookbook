//! One full conversational exchange for callers with exclusive store
//! access (the CLI, tests): append the user turn, assemble context, drive
//! the tool loop, persist the reply. The server inlines this flow instead,
//! locking the store only around the store-touching steps.

use mise_store::Store;
use mise_store::threads::{Role, ThreadId};

use crate::context;
use crate::error::{AssistantError, Result};
use crate::fetch::{self, Fetch};
use crate::recon::{self, Photo};
use crate::seam::Model;
use crate::tools::{self, ToolCtx};
use crate::turn::{Step, Turn};

/// The stamp for a thread's next message: `now`, pushed just past the
/// thread's latest stamp when civil time stalled or stepped backwards
/// (a DST fall-back, an NTP step). Thread order is (created, uid) and
/// drives both the export transcript and the history the model resumes,
/// so a new message must never sort before what came before it. Shared
/// by both drivers — one clamp, not two copies.
pub fn stamp_after(
    last: Option<jiff::civil::DateTime>,
    now: jiff::civil::DateTime,
) -> jiff::civil::DateTime {
    match last {
        Some(last) if now <= last => last.saturating_add(jiff::SignedDuration::from_nanos(1)),
        _ => now,
    }
}

/// Progress events for display: streamed text, tool activity, and recon
/// proposals on their way to being shown as tappable lines.
pub enum ExchangeEvent<'a> {
    TextDelta(&'a str),
    ToolCall { name: &'a str },
    Proposal(&'a recon::Proposal),
}

#[derive(Debug)]
pub struct Exchange {
    pub reply: String,
    /// Tool names in execution order — handy for provenance summaries.
    pub tools_used: Vec<String>,
}

/// `clock` is read once at the start (context, tool dates, commit stamps,
/// the user message's stamp) and once more when the reply persists —
/// thread order is (created, uid), so the reply must be stamped *after*
/// the message it answers. Still an input: tests script it.
// A driver entry point wears its inputs openly; a params struct here would
// be ceremony, not clarity.
#[allow(clippy::too_many_arguments)]
pub async fn run_exchange<M: Model, F: Fetch>(
    model: &mut M,
    fetcher: &mut F,
    store: &mut Store,
    thread: &ThreadId,
    user_message: &str,
    photos: &[Photo],
    clock: &mut (dyn FnMut() -> jiff::Zoned + Send),
    on_event: &mut (dyn FnMut(ExchangeEvent<'_>) + Send),
) -> Result<Exchange> {
    recon::validate_all(photos).map_err(AssistantError::Protocol)?;
    let now = clock();
    let ctx = ToolCtx { now: now.clone(), provenance: context::provenance(thread) };
    let now = now.datetime();
    // Photos ride only this exchange: the thread stores a counted
    // placeholder, and the image blocks are attached to the outgoing
    // turn. That turn is built locally and pushed explicitly — never
    // found by thread ordering, where a replica's fast clock may already
    // have stamped a message after this one.
    let stored = recon::transcript_text(user_message, photos.len());
    let (system, mut history) = context::assemble(store, thread, now)?;
    let asked = stamp_after(store.last_thread_stamp(thread)?, now);
    store.append_thread_message(thread, Role::User, &stored, asked)?;
    let mut outgoing = crate::seam::ChatMessage::user_text(stored);
    outgoing.content.splice(0..0, photos.iter().map(Photo::block));
    history.push(outgoing);

    let mut turn = Turn::new(system, history);
    let mut tools_used = Vec::new();
    let result: Result<String> = async {
        loop {
            let mut forward = |delta: &str| on_event(ExchangeEvent::TextDelta(delta));
            let model_turn = model.next_turn(turn.request(), &mut forward).await?;
            match turn.absorb(model_turn)? {
                Step::Done(reply) => break Ok(reply),
                Step::Execute(calls) => {
                    let mut outcomes = Vec::with_capacity(calls.len());
                    for call in &calls {
                        on_event(ExchangeEvent::ToolCall { name: &call.name });
                        tools_used.push(call.name.clone());
                        if call.name == fetch::FETCH_URL {
                            outcomes.push(fetch::execute_fetch(fetcher, call).await);
                        } else if call.name == recon::PROPOSE_PANTRY_DIFF {
                            let (outcome, proposal) = recon::execute_propose(call);
                            if let Some(p) = &proposal {
                                on_event(ExchangeEvent::Proposal(p));
                            }
                            outcomes.push(outcome);
                        } else {
                            outcomes.push(tools::execute(store, &ctx, call)?);
                        }
                    }
                    turn.provide(outcomes)?;
                }
            }
        }
    }
    .await;

    // Clamp against a stalled or backwards clock: whatever answers the
    // question — the reply, or a failure marker — must sort after it.
    let replied = stamp_after(Some(asked), clock().datetime());
    match result {
        Ok(reply) => {
            store.append_thread_message(thread, Role::Assistant, &reply, replied)?;
            Ok(Exchange { reply, tools_used })
        }
        Err(e) => {
            // The question is already persisted and earlier rounds may
            // have mutated the store. A marker keeps the thread from
            // dangling; best-effort, since the original error is the one
            // worth reporting. The caller owns the export and must run
            // it on this path too.
            let _ = store.append_thread_message(
                thread,
                Role::Assistant,
                &format!("(no reply — the exchange failed: {e})"),
                replied,
            );
            Err(e)
        }
    }
}
