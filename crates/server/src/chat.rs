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

use crate::{AppState, ChatConfig, ChatRequest};

type Tx = UnboundedSender<Event>;

fn send(tx: &Tx, event: &str, data: serde_json::Value) {
    // A dropped receiver just means the client went away mid-stream.
    let _ = tx.send(Event::default().event(event).data(data.to_string()));
}

pub(crate) async fn drive(
    state: AppState,
    config: std::sync::Arc<ChatConfig>,
    request: ChatRequest,
    tx: Tx,
) {
    match exchange(&state, &config, &request, &tx).await {
        Ok(()) => {}
        Err(e) => {
            warn!("chat exchange failed: {e}");
            send(&tx, "error", json!({"message": e.to_string()}));
        }
    }
}

async fn exchange(
    state: &AppState,
    config: &ChatConfig,
    request: &ChatRequest,
    tx: &Tx,
) -> Result<()> {
    use mise_assistant::AssistantError;

    let message = request.message.trim().to_string();
    let thread = match &request.page {
        Some(p) => ThreadId::Page(mise_store::DocId::parse(p)?),
        None => ThreadId::Planning,
    };

    let now = Zoned::now().datetime();
    let ctx = ToolCtx { now, provenance: provenance(&thread) };
    let (system, history) = {
        let mut store = state.store.lock().await;
        if let ThreadId::Page(id) = &thread
            && !store.exists(id)?
        {
            return Err(AssistantError::Protocol(format!("no page {id} to talk about")));
        }
        store.append_thread_message(&thread, Role::User, &message, now)?;
        context::assemble(&store, &thread, now)?
    };

    let mut client =
        AnthropicClient::new(config.api_key.clone()).with_model(config.model.clone());
    client = client.with_base_url(config.base_url.clone());
    let mut turn = Turn::new(system, history);
    let mut tools_used: Vec<String> = Vec::new();
    let reply = loop {
        let deltas = tx.clone();
        let mut on_delta =
            move |d: &str| send(&deltas, "delta", json!({"text": d}));
        let model_turn = client.next_turn(turn.request(), &mut on_delta).await?;
        match turn.absorb(model_turn)? {
            Step::Done(reply) => break reply,
            Step::Execute(calls) => {
                let mut store = state.store.lock().await;
                let mut outcomes = Vec::with_capacity(calls.len());
                for call in &calls {
                    send(tx, "tool", json!({"name": call.name}));
                    tools_used.push(call.name.clone());
                    outcomes.push(tools::execute(&mut store, &ctx, call)?);
                }
                drop(store);
                turn.provide(outcomes)?;
            }
        }
    };

    {
        let mut store = state.store.lock().await;
        if !reply.is_empty() {
            // Same monotonicity clamp as run_exchange: the reply must sort
            // after its question.
            let mut replied = Zoned::now().datetime();
            if replied <= now {
                replied = now.saturating_add(jiff::SignedDuration::from_nanos(1));
            }
            store.append_thread_message(&thread, Role::Assistant, &reply, replied)?;
        }
        let mut summary: String = message.chars().take(60).collect();
        if summary.len() < message.len() {
            summary.push('…');
        }
        store.export(&format!("{}: {summary}", provenance(&thread)))?;
    }
    info!("chat exchange done ({} tool calls)", tools_used.len());
    send(tx, "done", json!({"reply": reply, "tools_used": tools_used}));
    Ok(())
}
