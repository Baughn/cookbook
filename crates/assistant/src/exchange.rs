//! One full conversational exchange for callers with exclusive store
//! access (the CLI, tests): append the user turn, assemble context, drive
//! the tool loop, persist the reply. The server inlines this flow instead,
//! locking the store only around the store-touching steps.

use mise_store::Store;
use mise_store::threads::{Role, ThreadId};

use crate::context;
use crate::error::Result;
use crate::fetch::{self, Fetch};
use crate::seam::Model;
use crate::tools::{self, ToolCtx};
use crate::turn::{Step, Turn};

/// Progress events for display: streamed text and tool activity.
pub enum ExchangeEvent<'a> {
    TextDelta(&'a str),
    ToolCall { name: &'a str },
}

pub struct Exchange {
    pub reply: String,
    /// Tool names in execution order — handy for provenance summaries.
    pub tools_used: Vec<String>,
}

/// `clock` is read once at the start (context, tool dates, commit stamps,
/// the user message's stamp) and once more when the reply persists —
/// thread order is (created, uid), so the reply must be stamped *after*
/// the message it answers. Still an input: tests script it.
pub async fn run_exchange<M: Model, F: Fetch>(
    model: &mut M,
    fetcher: &mut F,
    store: &mut Store,
    thread: &ThreadId,
    user_message: &str,
    clock: &mut (dyn FnMut() -> jiff::Zoned + Send),
    on_event: &mut (dyn FnMut(ExchangeEvent<'_>) + Send),
) -> Result<Exchange> {
    let now = clock();
    let ctx = ToolCtx { now: now.clone(), provenance: context::provenance(thread) };
    let now = now.datetime();
    store.append_thread_message(thread, Role::User, user_message, now)?;
    let (system, history) = context::assemble(store, thread, now)?;

    let mut turn = Turn::new(system, history);
    let mut tools_used = Vec::new();
    let reply = loop {
        let mut forward = |delta: &str| on_event(ExchangeEvent::TextDelta(delta));
        let model_turn = model.next_turn(turn.request(), &mut forward).await?;
        match turn.absorb(model_turn)? {
            Step::Done(reply) => break reply,
            Step::Execute(calls) => {
                let mut outcomes = Vec::with_capacity(calls.len());
                for call in &calls {
                    on_event(ExchangeEvent::ToolCall { name: &call.name });
                    tools_used.push(call.name.clone());
                    if call.name == fetch::FETCH_URL {
                        outcomes.push(fetch::execute_fetch(fetcher, call).await);
                    } else {
                        outcomes.push(tools::execute(store, &ctx, call)?);
                    }
                }
                turn.provide(outcomes)?;
            }
        }
    };

    if !reply.is_empty() {
        // Clamp against a stalled or backwards clock: the reply must sort
        // after the message it answers.
        let mut replied = clock().datetime();
        if replied <= now {
            replied = now.saturating_add(jiff::SignedDuration::from_nanos(1));
        }
        store.append_thread_message(thread, Role::Assistant, &reply, replied)?;
    }
    Ok(Exchange { reply, tools_used })
}
