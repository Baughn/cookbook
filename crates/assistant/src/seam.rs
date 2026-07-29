//! The LLM seam: the one trait all model interaction sits behind, and the
//! request/turn types it speaks. These are *our* types — the Anthropic
//! client maps them to wire JSON; scripted fakes construct them directly.

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: Vec<ContentBlock>,
}

impl ChatMessage {
    pub fn user_text(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

/// Everything the model needs to produce the next turn. The system prompt
/// leads and stays stable within a session (prompt-cache friendly); the
/// volatile conversation tail comes last.
#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

/// One assistant turn as the model produced it: text and/or tool calls.
#[derive(Clone, Debug)]
pub struct ModelTurn {
    pub content: Vec<ContentBlock>,
    pub stop: StopReason,
}

/// The seam. Implementations: the real Anthropic client, and scripted fakes
/// in tests. `on_delta` receives streamed text as it arrives — advisory,
/// display-only; the returned turn is authoritative.
pub trait Model {
    fn next_turn(
        &mut self,
        req: &TurnRequest,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> impl Future<Output = Result<ModelTurn>> + Send;
}
