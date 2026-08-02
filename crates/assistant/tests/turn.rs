//! The turn state machine driven by scripted model turns — the whole tool
//! loop with no model and no IO, per the testing charter.

use jiff::civil::DateTime;
use mise_assistant::AssistantError;
use mise_assistant::seam::{ContentBlock, ModelTurn, StopReason};
use mise_assistant::tools::{ToolCtx, execute};
use mise_assistant::turn::{Step, ToolOutcome, Turn};
use mise_assistant::seam::ChatMessage;
use mise_core::types::Slug;
use mise_store::pages::QueueDoc;
use mise_store::{DocId, Store};
use serde_json::json;

fn text(t: &str) -> ContentBlock {
    ContentBlock::Text { text: t.into() }
}

fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolUse { id: id.into(), name: name.into(), input }
}

fn turn_with(user: &str) -> Turn {
    Turn::new("system prompt".into(), vec![ChatMessage::user_text(user)])
}

fn outcome(id: &str, content: &str) -> ToolOutcome {
    ToolOutcome { tool_use_id: id.into(), content: content.into(), is_error: false }
}

#[test]
fn text_only_turn_finishes() {
    let mut turn = turn_with("hello");
    let step = turn
        .absorb(ModelTurn { content: vec![text("Hi there.")], stop: StopReason::EndTurn })
        .unwrap();
    assert_eq!(step, Step::Done("Hi there.".into()));
}

#[test]
fn tool_round_trip_accumulates_narration() {
    let mut turn = turn_with("what's on the queue?");
    let step = turn
        .absorb(ModelTurn {
            content: vec![
                text("Let me look."),
                tool_use("c1", "queue_status", json!({})),
            ],
            stop: StopReason::ToolUse,
        })
        .unwrap();
    let Step::Execute(calls) = step else { panic!("expected Execute") };
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "queue_status");

    turn.provide(vec![outcome("c1", "Queue — home …")]).unwrap();
    let step = turn
        .absorb(ModelTurn { content: vec![text("It's empty.")], stop: StopReason::EndTurn })
        .unwrap();
    assert_eq!(step, Step::Done("Let me look.\n\nIt's empty.".into()));

    // The request now holds the full exchange for the next model call.
    assert_eq!(turn.request().messages.len(), 4);
}

#[test]
fn thinking_stays_in_the_conversation_but_out_of_the_reply() {
    let mut turn = turn_with("plan something");
    let step = turn
        .absorb(ModelTurn {
            content: vec![
                ContentBlock::Thinking { thinking: "curry twice already".into(), signature: "s".into() },
                text("Dal, then."),
            ],
            stop: StopReason::EndTurn,
        })
        .unwrap();
    assert_eq!(step, Step::Done("Dal, then.".into()), "reasoning is not reply text");
    // But the block survives in the request for the next model call.
    let last = turn.request().messages.last().unwrap();
    assert!(matches!(last.content[0], ContentBlock::Thinking { .. }));
}

#[test]
fn outcomes_must_match_pending_calls() {
    let mut turn = turn_with("x");
    turn.absorb(ModelTurn {
        content: vec![tool_use("c1", "queue_status", json!({}))],
        stop: StopReason::ToolUse,
    })
    .unwrap();
    let e = turn.provide(vec![outcome("c2", "nope")]).unwrap_err();
    assert!(matches!(e, AssistantError::Protocol(_)), "{e}");
}

#[test]
fn absorbing_while_results_are_owed_is_a_protocol_error() {
    let mut turn = turn_with("x");
    turn.absorb(ModelTurn {
        content: vec![tool_use("c1", "queue_status", json!({}))],
        stop: StopReason::ToolUse,
    })
    .unwrap();
    let e = turn
        .absorb(ModelTurn { content: vec![text("hi")], stop: StopReason::EndTurn })
        .unwrap_err();
    assert!(matches!(e, AssistantError::Protocol(_)), "{e}");
}

#[test]
fn model_sent_tool_result_is_rejected() {
    let mut turn = turn_with("x");
    let e = turn
        .absorb(ModelTurn {
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: "?".into(),
                is_error: false,
            }],
            stop: StopReason::EndTurn,
        })
        .unwrap_err();
    assert!(matches!(e, AssistantError::Protocol(_)), "{e}");
}

#[test]
fn max_tokens_ends_the_exchange_even_with_calls_and_says_so() {
    let mut turn = turn_with("x");
    let step = turn
        .absorb(ModelTurn {
            content: vec![text("So far…"), tool_use("c1", "queue_status", json!({}))],
            stop: StopReason::MaxTokens,
        })
        .unwrap();
    // What text we have, but never presented as a complete answer.
    let Step::Done(reply) = step else { panic!("expected Done") };
    assert!(reply.starts_with("So far…"), "{reply}");
    assert!(reply.contains("cut short"), "a truncated turn must report truncation: {reply}");
}

#[test]
fn a_truncated_turn_with_no_text_is_an_error_not_an_empty_reply() {
    let mut turn = turn_with("x");
    let e = turn
        .absorb(ModelTurn {
            content: vec![tool_use("c1", "queue_status", json!({}))],
            stop: StopReason::MaxTokens,
        })
        .unwrap_err();
    assert!(e.to_string().contains("cut"), "{e}");
}

#[test]
fn an_empty_reply_is_an_error_not_a_success() {
    // A turn that ends the exchange having said nothing is a failure the
    // caller must see — not `done {"reply":""}` and a dangling question.
    let mut turn = turn_with("x");
    let e = turn
        .absorb(ModelTurn { content: vec![], stop: StopReason::EndTurn })
        .unwrap_err();
    assert!(e.to_string().contains("without a reply"), "{e}");
}

#[test]
fn runaway_tool_loop_is_cut_off() {
    let mut turn = turn_with("x");
    let e = loop {
        match turn.absorb(ModelTurn {
            content: vec![tool_use("c", "queue_status", json!({}))],
            stop: StopReason::ToolUse,
        }) {
            Ok(Step::Execute(_)) => turn.provide(vec![outcome("c", "…")]).unwrap(),
            Ok(Step::Done(_)) => panic!("loop should not finish"),
            Err(e) => break e,
        }
    };
    assert!(matches!(e, AssistantError::Protocol(_)), "{e}");
}

/// The round cap is a backstop, not a shredder: when the loop has been
/// narrating along the way, cutting it off yields what was said plus a
/// note, not a protocol error that throws the narration away.
#[test]
fn a_runaway_loop_ends_with_what_was_said_and_a_note() {
    let mut turn = turn_with("x");
    let reply = loop {
        match turn.absorb(ModelTurn {
            content: vec![text("still looking…"), tool_use("c", "queue_status", json!({}))],
            stop: StopReason::ToolUse,
        }) {
            Ok(Step::Execute(_)) => turn.provide(vec![outcome("c", "…")]).unwrap(),
            Ok(Step::Done(reply)) => break reply,
            Err(e) => panic!("a narrated loop ends with a reply, not {e}"),
        }
    };
    assert!(reply.contains("still looking…"), "{reply}");
    assert!(reply.contains("stopped"), "the cut-off is visible: {reply}");
}

/// The composed picture: scripted model turns whose tool calls actually
/// execute against a real store — a mini planning exchange, no model.
#[test]
fn scripted_exchange_mutates_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    let ctx = ToolCtx {
        now: DateTime::constant(2026, 7, 29, 18, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap(),
        provenance: "planning thread".into(),
    };

    let script = [
        ModelTurn {
            content: vec![
                text("Checking the queue first."),
                tool_use("c1", "queue_status", json!({})),
            ],
            stop: StopReason::ToolUse,
        },
        ModelTurn {
            content: vec![tool_use(
                "c2",
                "queue_add",
                json!({"title": "Dal with flatbread", "reason": "cheap, keeps well"}),
            )],
            stop: StopReason::ToolUse,
        },
        ModelTurn {
            content: vec![text("Queued dal for this week.")],
            stop: StopReason::EndTurn,
        },
    ];

    let mut turn = turn_with("plan something cheap");
    let mut reply = None;
    for scripted in script {
        match turn.absorb(scripted).unwrap() {
            Step::Execute(calls) => {
                let outcomes = calls
                    .iter()
                    .map(|c| execute(&mut store, &ctx, c).unwrap())
                    .collect();
                turn.provide(outcomes).unwrap();
            }
            Step::Done(text) => reply = Some(text),
        }
    }
    assert_eq!(reply.unwrap(), "Checking the queue first.\n\nQueued dal for this week.");

    let queue: QueueDoc = store.get(&DocId::Queue).unwrap();
    let entry = &queue.entries["dal-with-flatbread"];
    assert_eq!(entry.reason.as_deref(), Some("cheap, keeps well"));
    assert_eq!(entry.added, "2026-07-29");
}
