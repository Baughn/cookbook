//! The turn driver: a sans-IO state machine for one conversational
//! exchange. The caller shuttles model turns in and tool results back:
//!
//! ```text
//! let mut turn = Turn::new(system, history);
//! loop {
//!     match turn.absorb(model.next_turn(turn.request(), …).await?)? {
//!         Step::Execute(calls) => {
//!             // lock the store only here
//!             let results = calls.iter().map(|c| tools::execute(store, &ctx, c)).…;
//!             turn.provide(results)?;
//!         }
//!         Step::Done(reply) => break,
//!     }
//! }
//! ```
//!
//! Keeping IO out makes the loop testable with scripted fakes and leaves
//! lock discipline to the caller: model calls never hold the store.

use crate::error::{AssistantError, Result};
use crate::seam::{ChatMessage, ChatRole, ContentBlock, ModelTurn, StopReason, TurnRequest};

/// Hard cap on tool rounds per exchange — a runaway-loop backstop, far above
/// any real planning session.
const MAX_TOOL_ROUNDS: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Execute these calls against the store, then [`Turn::provide`] the
    /// outcomes in the same order.
    Execute(Vec<ToolCall>),
    /// The exchange is over; here is the assistant's visible reply.
    Done(String),
}

pub struct Turn {
    req: TurnRequest,
    reply: String,
    pending: Vec<ToolCall>,
    rounds: usize,
}

impl Turn {
    /// `messages` is the conversation so far, ending with the user message
    /// that opens this exchange.
    pub fn new(system: String, messages: Vec<ChatMessage>) -> Turn {
        Turn {
            req: TurnRequest { system, messages, tools: crate::tools::tool_defs() },
            reply: String::new(),
            pending: Vec::new(),
            rounds: 0,
        }
    }

    pub fn request(&self) -> &TurnRequest {
        &self.req
    }

    /// Take in one model turn. Text accumulates into the visible reply
    /// (interstitial narration included — it reads well in transcripts);
    /// tool calls come back as [`Step::Execute`].
    pub fn absorb(&mut self, turn: ModelTurn) -> Result<Step> {
        if !self.pending.is_empty() {
            return Err(AssistantError::Protocol(
                "absorb called with tool results still owing".into(),
            ));
        }
        let mut calls = Vec::new();
        for block in &turn.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text.trim().is_empty() {
                        if !self.reply.is_empty() {
                            self.reply.push_str("\n\n");
                        }
                        self.reply.push_str(text.trim());
                    }
                }
                ContentBlock::ToolUse { id, name, input } => calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                ContentBlock::ToolResult { .. } | ContentBlock::Image { .. } => {
                    return Err(AssistantError::Protocol(
                        "model turn contained a non-model block".into(),
                    ));
                }
            }
        }
        self.req.messages.push(ChatMessage { role: ChatRole::Assistant, content: turn.content });

        // A stop for any reason other than tool use ends the exchange, calls
        // or no calls: `max_tokens` mid-call yields what text we have rather
        // than a half-executed round.
        if turn.stop != StopReason::ToolUse || calls.is_empty() {
            return Ok(Step::Done(self.reply.clone()));
        }
        self.rounds += 1;
        if self.rounds > MAX_TOOL_ROUNDS {
            return Err(AssistantError::Protocol(format!(
                "tool loop exceeded {MAX_TOOL_ROUNDS} rounds"
            )));
        }
        self.pending = calls.clone();
        Ok(Step::Execute(calls))
    }

    /// Hand back the outcomes for the calls the last [`Step::Execute`]
    /// requested, in order.
    pub fn provide(&mut self, outcomes: Vec<ToolOutcome>) -> Result<()> {
        let expected: Vec<&str> = self.pending.iter().map(|c| c.id.as_str()).collect();
        let got: Vec<&str> = outcomes.iter().map(|o| o.tool_use_id.as_str()).collect();
        if expected != got {
            return Err(AssistantError::Protocol(format!(
                "tool outcomes {got:?} do not match pending calls {expected:?}"
            )));
        }
        self.pending.clear();
        self.req.messages.push(ChatMessage {
            role: ChatRole::User,
            content: outcomes
                .into_iter()
                .map(|o| ContentBlock::ToolResult {
                    tool_use_id: o.tool_use_id,
                    content: o.content,
                    is_error: o.is_error,
                })
                .collect(),
        });
        Ok(())
    }
}
