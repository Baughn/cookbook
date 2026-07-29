//! run_exchange with a scripted Model: the exclusive-store driver persists
//! both turns to the thread, executes tools, and reports events.

use jiff::civil::DateTime;
use mise_assistant::exchange::{ExchangeEvent, run_exchange};
use mise_assistant::seam::{ContentBlock, Model, ModelTurn, StopReason, TurnRequest};
use mise_core::types::Slug;
use mise_store::pages::QueueDoc;
use mise_store::threads::{Role, ThreadId};
use mise_store::{DocId, Store};
use serde_json::json;

struct Scripted {
    turns: Vec<ModelTurn>,
    /// Snapshot of each request, for asserting what the model saw.
    seen: Vec<TurnRequest>,
}

impl Model for Scripted {
    async fn next_turn(
        &mut self,
        req: &TurnRequest,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> mise_assistant::error::Result<ModelTurn> {
        self.seen.push(req.clone());
        let turn = self.turns.remove(0);
        for block in &turn.content {
            if let ContentBlock::Text { text } = block {
                on_delta(text);
            }
        }
        Ok(turn)
    }
}

/// A scripted clock ticking one second per reading.
fn ticking() -> impl FnMut() -> DateTime + Send {
    let mut s = 0i8;
    move || {
        s += 1;
        DateTime::constant(2026, 7, 29, 18, 0, s, 0)
    }
}

#[tokio::test]
async fn exchange_persists_thread_executes_tools_and_streams() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2).unwrap();
    let mut model = Scripted {
        turns: vec![
            ModelTurn {
                content: vec![ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "queue_add".into(),
                    input: json!({"title": "Dal", "reason": "cheap"}),
                }],
                stop: StopReason::ToolUse,
            },
            ModelTurn {
                content: vec![ContentBlock::Text { text: "Queued dal.".into() }],
                stop: StopReason::EndTurn,
            },
        ],
        seen: vec![],
    };

    let mut deltas = String::new();
    let mut tool_names = Vec::new();
    let exchange = run_exchange(
        &mut model,
        &mut store,
        &ThreadId::Planning,
        "plan something cheap",
        &mut ticking(),
        &mut |e| match e {
            ExchangeEvent::TextDelta(d) => deltas.push_str(d),
            ExchangeEvent::ToolCall { name } => tool_names.push(name.to_string()),
        },
    )
    .await
    .unwrap();

    assert_eq!(exchange.reply, "Queued dal.");
    assert_eq!(exchange.tools_used, vec!["queue_add"]);
    assert_eq!(deltas, "Queued dal.");
    assert_eq!(tool_names, vec!["queue_add"]);

    // The store took the edit, with thread provenance driving the export
    // messages later; both turns are in the thread.
    let queue: QueueDoc = store.get(&DocId::Queue).unwrap();
    assert!(queue.entries.contains_key("dal"));
    let msgs = store.thread_messages(&ThreadId::Planning).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[0].content, "plan something cheap");
    assert_eq!(msgs[1].role, Role::Assistant);
    assert_eq!(msgs[1].content, "Queued dal.");
    assert!(msgs[1].created > msgs[0].created, "the reply sorts after its question");

    // The model saw the user message as the last conversation turn, and the
    // corpus in the system prompt.
    let first_req = &model.seen[0];
    assert!(first_req.system.contains("## The corpus now"));
    assert_eq!(first_req.messages.len(), 1);

    // A follow-up exchange sees the persisted history.
    let mut model2 = Scripted {
        turns: vec![ModelTurn {
            content: vec![ContentBlock::Text { text: "Already queued.".into() }],
            stop: StopReason::EndTurn,
        }],
        seen: vec![],
    };
    let mut later = {
        let mut s = 0i8;
        move || {
            s += 1;
            DateTime::constant(2026, 7, 29, 19, 0, s, 0)
        }
    };
    run_exchange(&mut model2, &mut store, &ThreadId::Planning, "queue dal?", &mut later, &mut |_| {})
        .await
        .unwrap();
    assert_eq!(model2.seen[0].messages.len(), 3, "prior turns + new user message");
}

/// A clock that stalls (or steps backwards) still yields a transcript in
/// conversation order: the reply's stamp is clamped past the question's.
#[tokio::test]
async fn stalled_clock_cannot_invert_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2).unwrap();
    let mut model = Scripted {
        turns: vec![ModelTurn {
            content: vec![ContentBlock::Text { text: "Sure.".into() }],
            stop: StopReason::EndTurn,
        }],
        seen: vec![],
    };
    let mut frozen = || DateTime::constant(2026, 7, 29, 18, 0, 0, 0);
    run_exchange(&mut model, &mut store, &ThreadId::Planning, "hello?", &mut frozen, &mut |_| {})
        .await
        .unwrap();
    let msgs = store.thread_messages(&ThreadId::Planning).unwrap();
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert!(msgs[1].created > msgs[0].created);
}
