//! The server side of one chat exchange. This mirrors
//! `mise_assistant::exchange::run_exchange` but drives the sans-IO `Turn`
//! directly, because the server's store sits behind a mutex: the lock is
//! taken for the store-touching steps and released before every model
//! call.

use axum::response::sse::Event;
use jiff::Zoned;
use mise_assistant::client::AnthropicClient;
use mise_assistant::context::{self, provenance};
use mise_assistant::error::Result;
use mise_assistant::seam::Model;
use mise_assistant::tools::{self, ToolCtx};
use mise_assistant::turn::{Step, Turn};
use mise_store::threads::{Role, ThreadId};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::{AppState, ChatRequest};

type Tx = UnboundedSender<Event>;

fn send(tx: &Tx, event: &str, data: serde_json::Value) {
    // A dropped receiver just means the client went away mid-stream.
    let _ = tx.send(Event::default().event(event).data(data.to_string()));
}

pub(crate) async fn drive(
    state: AppState,
    client: AnthropicClient,
    request: ChatRequest,
    tx: Tx,
) {
    match exchange(&state, client, &request, &tx).await {
        Ok(()) => {}
        Err(e) => {
            warn!("chat exchange failed: {e}");
            send(&tx, "error", json!({"message": e.to_string()}));
        }
    }
}

async fn exchange(
    state: &AppState,
    mut client: AnthropicClient,
    request: &ChatRequest,
    tx: &Tx,
) -> Result<()> {
    use mise_assistant::AssistantError;
    use mise_assistant::recon;

    let message = request.message.trim().to_string();
    let thread = match &request.page {
        Some(p) => mise_store::ThreadId::parse(p)?,
        None => ThreadId::Planning,
    };
    let photos: Vec<recon::Photo> = request
        .images
        .iter()
        .map(|i| recon::Photo { media_type: i.media_type.clone(), data: i.data.clone() })
        .collect();
    recon::validate_all(&photos).map_err(AssistantError::Protocol)?;

    let started = Zoned::now();
    let ctx = ToolCtx { now: started.clone(), provenance: provenance(&thread) };
    let now = started.datetime();
    let (system, history, asked) = {
        let mut store = state.store.lock().await;
        if let ThreadId::Page(id) = &thread
            && !store.exists(id)?
        {
            return Err(AssistantError::Protocol(format!("no page {id} to talk about")));
        }
        // Photos ride only this exchange: the thread stores a counted
        // placeholder, the image blocks go on the wire. The outgoing turn
        // is built locally and pushed explicitly — never found by thread
        // ordering, where a replica's fast clock may already have stamped
        // a message after this one (same shape as run_exchange).
        let stored = recon::transcript_text(&message, photos.len());
        let (system, mut history) = context::assemble(&store, &thread, now)?;
        // Same clamp as run_exchange: the question must sort after
        // everything already on the thread, even when civil time stepped
        // backwards since the last message was stamped.
        let asked = mise_assistant::exchange::stamp_after(store.last_thread_stamp(&thread)?, now);
        store.append_thread_message(&thread, Role::User, &stored, asked)?;
        let mut outgoing = mise_assistant::seam::ChatMessage::user_text(stored);
        outgoing.content.splice(0..0, photos.iter().map(recon::Photo::block));
        history.push(outgoing);
        (system, history, asked)
    };

    let mut turn = Turn::new(system, history);
    let mut fetcher = mise_assistant::fetch::HttpFetch::new();
    let mut tools_used: Vec<String> = Vec::new();
    let result: Result<String> = async { loop {
        let deltas = tx.clone();
        let mut on_delta =
            move |d: &str| send(&deltas, "delta", json!({"text": d}));
        let model_turn = client.next_turn(turn.request(), &mut on_delta).await?;
        match turn.absorb(model_turn)? {
            Step::Done(reply) => break Ok(reply),
            Step::Execute(calls) => {
                let mut outcomes = Vec::with_capacity(calls.len());
                for call in &calls {
                    send(tx, "tool", json!({"name": call.name}));
                    tools_used.push(call.name.clone());
                    if call.name == mise_assistant::fetch::FETCH_URL {
                        // The network never holds the store lock.
                        outcomes
                            .push(mise_assistant::fetch::execute_fetch(&mut fetcher, call).await);
                    } else if call.name == recon::PROPOSE_PANTRY_DIFF {
                        // Validated, forwarded to the UI as tappable
                        // lines, never applied here. The latest proposal
                        // also parks in memory so its taps survive the
                        // exchange — see AppState::proposals.
                        let (outcome, proposal) = recon::execute_propose(call);
                        // Recon itself never touches the store, but a
                        // proposal for a location the store doesn't know
                        // can never be applied or completed — every tap
                        // would 400 and the parked entry would never be
                        // dropped. Turn the miss into a model-facing
                        // error so it retries with a real location.
                        let unknown = match &proposal {
                            Some(p) => match &p.location {
                                Some(l) => {
                                    let store = state.store.lock().await;
                                    let s: mise_store::pages::StateDoc =
                                        store.get(&mise_store::DocId::State)?;
                                    (!s.locations.contains_key(l.as_str())).then(|| l.clone())
                                }
                                None => None,
                            },
                            None => None,
                        };
                        if let Some(l) = unknown {
                            outcomes.push(mise_assistant::turn::ToolOutcome {
                                tool_use_id: call.id.clone(),
                                content: format!("no location {l}"),
                                is_error: true,
                            });
                        } else {
                            if let Some(p) = &proposal {
                                send(tx, "proposal", json!(p));
                                state.proposals.lock().await.insert(thread.to_string(), p.clone());
                            }
                            outcomes.push(outcome);
                        }
                    } else {
                        let mut store = state.store.lock().await;
                        outcomes.push(tools::execute(&mut store, &ctx, call)?);
                    }
                }
                turn.provide(outcomes)?;
            }
        }
    } }
    .await;

    let reply = {
        let mut store = state.store.lock().await;
        // Same monotonicity clamp as run_exchange: whatever answers the
        // question — the reply, or a failure marker — must sort after it.
        let replied = mise_assistant::exchange::stamp_after(Some(asked), Zoned::now().datetime());
        let mut summary: String = if message.is_empty() && !photos.is_empty() {
            "[photo]".to_string()
        } else {
            message.chars().take(60).collect()
        };
        if summary.len() < message.len() {
            summary.push('…');
        }
        match result {
            Ok(reply) => {
                store.append_thread_message(&thread, Role::Assistant, &reply, replied)?;
                store.export(&format!("{}: {summary}", provenance(&thread)))?;
                reply
            }
            Err(e) => {
                // The question is persisted and earlier rounds may have
                // mutated the store: mark the thread and export anyway,
                // best-effort, so the readable backup never sits behind
                // the store. The original error still reaches the client.
                let _ = store.append_thread_message(
                    &thread,
                    Role::Assistant,
                    &format!("(no reply — the exchange failed: {e})"),
                    replied,
                );
                let _ = store.export(&format!("{} (failed): {summary}", provenance(&thread)));
                return Err(e);
            }
        }
    };
    info!("chat exchange done ({} tool calls)", tools_used.len());
    send(tx, "done", json!({"reply": reply, "tools_used": tools_used}));
    Ok(())
}
