//! run_exchange with a scripted Model: the exclusive-store driver persists
//! both turns to the thread, executes tools, and reports events.

use jiff::civil::DateTime;
use mise_assistant::exchange::{ExchangeEvent, run_exchange};
use mise_assistant::fetch::Fetch;
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

/// Scripted network: url → HTML, no network anywhere near the tests.
struct ScriptedFetch(std::collections::BTreeMap<String, String>);

impl Fetch for ScriptedFetch {
    async fn fetch(&mut self, url: &str) -> Result<String, String> {
        self.0.get(url).cloned().ok_or_else(|| format!("no route to {url}"))
    }
}

fn no_fetch() -> ScriptedFetch {
    ScriptedFetch(std::collections::BTreeMap::new())
}

/// A scripted clock ticking one second per reading.
fn ticking() -> impl FnMut() -> jiff::Zoned + Send {
    let mut s = 0i8;
    move || {
        s += 1;
        DateTime::constant(2026, 7, 29, 18, 0, s, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
    }
}

#[tokio::test]
async fn exchange_persists_thread_executes_tools_and_streams() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
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
        &mut no_fetch(),
        &mut store,
        &ThreadId::Planning,
        "plan something cheap",
        &[],
        &mut ticking(),
        &mut |e| match e {
            ExchangeEvent::TextDelta(d) => deltas.push_str(d),
            ExchangeEvent::ToolCall { name } => tool_names.push(name.to_string()),
            ExchangeEvent::Proposal(_) => unreachable!("no recon in this exchange"),
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
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap()
        }
    };
    run_exchange(&mut model2, &mut no_fetch(), &mut store, &ThreadId::Planning, "queue dal?", &[], &mut later, &mut |_| {})
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
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    let mut model = Scripted {
        turns: vec![ModelTurn {
            content: vec![ContentBlock::Text { text: "Sure.".into() }],
            stop: StopReason::EndTurn,
        }],
        seen: vec![],
    };
    let mut frozen = || {
        DateTime::constant(2026, 7, 29, 18, 0, 0, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap()
    };
    run_exchange(&mut model, &mut no_fetch(), &mut store, &ThreadId::Planning, "hello?", &[], &mut frozen, &mut |_| {})
        .await
        .unwrap();
    let msgs = store.thread_messages(&ThreadId::Planning).unwrap();
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert!(msgs[1].created > msgs[0].created);
}

/// fetch_url is intercepted by the driver and never touches the store:
/// the scripted network's page comes back extracted, an unknown URL comes
/// back as an error result, and the exchange carries on.
#[tokio::test]
async fn fetch_url_flows_through_the_scripted_network() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    let mut model = Scripted {
        turns: vec![
            ModelTurn {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "f1".into(),
                        name: "fetch_url".into(),
                        input: json!({"url": "https://example.com/mapo"}),
                    },
                    ContentBlock::ToolUse {
                        id: "f2".into(),
                        name: "fetch_url".into(),
                        input: json!({"url": "https://example.com/nowhere"}),
                    },
                ],
                stop: StopReason::ToolUse,
            },
            ModelTurn {
                content: vec![ContentBlock::Text { text: "Drafted.".into() }],
                stop: StopReason::EndTurn,
            },
        ],
        seen: vec![],
    };
    let mut net = ScriptedFetch(
        [(
            "https://example.com/mapo".to_string(),
            r#"<html><head><script type="application/ld+json">
               {"@type":"Recipe","name":"Mapo tofu",
                "recipeIngredient":["tofu"],"recipeInstructions":["Cook it."]}
               </script></head><body>filler</body></html>"#
                .to_string(),
        )]
        .into(),
    );

    run_exchange(
        &mut model,
        &mut net,
        &mut store,
        &ThreadId::Planning,
        "have a look at https://example.com/mapo",
        &[],
        &mut ticking(),
        &mut |_| {},
    )
    .await
    .unwrap();

    // The second request carries both tool results: extracted markdown for
    // the hit, an error for the miss.
    let results: Vec<_> = model.seen[1].messages
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, is_error, .. } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].0.starts_with("# Mapo tofu"), "{}", results[0].0);
    assert!(results[0].0.contains("1. Cook it."));
    assert!(!results[0].1);
    assert!(results[1].0.contains("no route"), "{}", results[1].0);
    assert!(results[1].1, "a failed fetch is the model's problem, not an abort");
}

/// The recon flow, end to end below the seam: photos ride only the live
/// exchange (counted placeholder in the thread, image blocks on the wire,
/// gone by the follow-up — a shelf rarely fits one frame, so two here),
/// the proposal is validated and surfaced as an event, and the store is
/// never touched by it.
#[tokio::test]
async fn photo_recon_proposes_without_touching_the_store() {
    use mise_assistant::recon::{Photo, Proposal};
    use mise_store::pages::PantryDoc;

    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    let mut model = Scripted {
        turns: vec![
            ModelTurn {
                content: vec![ContentBlock::ToolUse {
                    id: "p1".into(),
                    name: "propose_pantry_diff".into(),
                    input: json!({"location": "home", "lines": [
                        {"item": "Silken Tofu", "presence": "have", "reason": "two packs, front"},
                        {"item": "miso", "presence": "out", "reason": "no jar visible"},
                    ]}),
                }],
                stop: StopReason::ToolUse,
            },
            ModelTurn {
                content: vec![ContentBlock::Text {
                    text: "Proposed 2 updates — tofu in, miso out.".into(),
                }],
                stop: StopReason::EndTurn,
            },
        ],
        seen: vec![],
    };

    let photos = [
        Photo { media_type: "image/jpeg".into(), data: "QUJD".into() },
        Photo { media_type: "image/png".into(), data: "REVG".into() },
    ];
    let mut proposals: Vec<Proposal> = Vec::new();
    run_exchange(
        &mut model,
        &mut no_fetch(),
        &mut store,
        &ThreadId::Page(DocId::parse("location/home/pantry").unwrap()),
        "here's the shelf",
        &photos,
        &mut ticking(),
        &mut |e| {
            if let ExchangeEvent::Proposal(p) = e {
                proposals.push(p.clone());
            }
        },
    )
    .await
    .unwrap();

    // The proposal came through validated (slugs normalized), and nothing
    // was written to the pantry.
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].location.as_deref(), Some("home"));
    assert_eq!(proposals[0].lines[0].item, "silken-tofu");
    let pantry: PantryDoc = store.get(&DocId::parse("location/home/pantry").unwrap()).unwrap();
    assert!(pantry.items.is_empty(), "a proposal must never touch the store");

    // The wire saw all the pixels, in order; the thread saw a count.
    let sent = model.seen[0].messages.last().unwrap();
    assert!(matches!(&sent.content[0], ContentBlock::Image { media_type, .. } if media_type == "image/jpeg"));
    assert!(matches!(&sent.content[1], ContentBlock::Image { media_type, .. } if media_type == "image/png"));
    assert!(matches!(&sent.content[2], ContentBlock::Text { .. }));
    let thread = ThreadId::Page(DocId::parse("location/home/pantry").unwrap());
    let msgs = store.thread_messages(&thread).unwrap();
    assert_eq!(msgs[0].content, "here's the shelf\n\n[2 photos attached]");

    // A follow-up (the correction turn) re-reads history without the
    // image: pixels never outlive their exchange.
    let mut model2 = Scripted {
        turns: vec![ModelTurn {
            content: vec![ContentBlock::Text { text: "Fixed.".into() }],
            stop: StopReason::EndTurn,
        }],
        seen: vec![],
    };
    let mut later = {
        let mut s = 0i8;
        move || {
            s += 1;
            DateTime::constant(2026, 7, 29, 19, 0, s, 0)
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap()
        }
    };
    run_exchange(&mut model2, &mut no_fetch(), &mut store, &thread, "you missed the rice", &[], &mut later, &mut |_| {})
        .await
        .unwrap();
    let images = model2.seen[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::Image { .. }))
        .count();
    assert_eq!(images, 0, "the photo is transient; only its placeholder persists");
}

/// A malformed proposal comes back as an error tool result — the model's
/// problem, not an abort, and still nothing lands in the store.
#[tokio::test]
async fn malformed_proposals_bounce_back_to_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut store =
        Store::create(&dir.path().join("corpus"), &Slug::new("home").unwrap(), 2, jiff::Timestamp::UNIX_EPOCH).unwrap();
    let mut model = Scripted {
        turns: vec![
            ModelTurn {
                content: vec![ContentBlock::ToolUse {
                    id: "p1".into(),
                    name: "propose_pantry_diff".into(),
                    input: json!({"lines": [{"item": "miso", "presence": "gone", "reason": "?"}]}),
                }],
                stop: StopReason::ToolUse,
            },
            ModelTurn {
                content: vec![ContentBlock::Text { text: "Let me retry.".into() }],
                stop: StopReason::EndTurn,
            },
        ],
        seen: vec![],
    };
    run_exchange(
        &mut model,
        &mut no_fetch(),
        &mut store,
        &ThreadId::Planning,
        "shelf photo",
        &[],
        &mut ticking(),
        &mut |e| {
            assert!(!matches!(e, ExchangeEvent::Proposal(_)), "bad proposals never surface");
        },
    )
    .await
    .unwrap();
    let (content, is_error) = match &model.seen[1].messages.last().unwrap().content[0] {
        ContentBlock::ToolResult { content, is_error, .. } => (content.clone(), *is_error),
        other => panic!("expected a tool result, got {other:?}"),
    };
    assert!(is_error);
    assert!(content.contains("bad presence"), "{content}");
}
