//! The hand-rolled Anthropic Messages client: wire types, SSE streaming,
//! prompt-caching markers. No community crate — the surface we use is
//! small and pinning it ourselves keeps API drift visible.
//!
//! Everything that can be tested without a network — request-body mapping,
//! SSE framing, stream assembly — is a pure function or struct with unit
//! tests below. The HTTP call itself is the thin remainder, exercised by
//! `evals/` and real use, never by the test suite.

use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::error::{AssistantError, Result};
use crate::seam::{ChatMessage, ChatRole, ContentBlock, Model, ModelTurn, StopReason, TurnRequest};

/// Pinned protocol version; bump deliberately, watching the changelog.
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-opus-5";
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const MAX_TOKENS: u32 = 8192;

pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    pub model: String,
}

impl AnthropicClient {
    pub fn new(api_key: impl Into<String>) -> AnthropicClient {
        AnthropicClient {
            http: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Point at a different endpoint (evals against a proxy, tests).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> AnthropicClient {
        self.base_url = base_url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> AnthropicClient {
        self.model = model.into();
        self
    }
}

impl Model for AnthropicClient {
    async fn next_turn(
        &mut self,
        req: &TurnRequest,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ModelTurn> {
        let body = request_body(&self.model, req);
        let response = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| AssistantError::Api(format!("request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<Value>(&detail)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or(detail);
            return Err(AssistantError::Api(format!("{status}: {message}")));
        }

        let mut frames = SseFrames::default();
        let mut assembler = Assembler::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| AssistantError::Api(format!("stream broke: {e}")))?;
            for frame in frames.push(&chunk) {
                if let Some(delta) = assembler.handle(&frame)? {
                    on_delta(&delta);
                }
            }
        }
        assembler.finish()
    }
}

// ------------------------------------------------------------- requests --

/// Our seam types → the Messages API body. The system prompt and the tool
/// list get `cache_control` markers: they are the stable prefix, and the
/// conversation tail is what varies turn to turn.
fn request_body(model: &str, req: &TurnRequest) -> Value {
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect();
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "system": [{
            "type": "text",
            "text": req.system,
            "cache_control": {"type": "ephemeral"},
        }],
        "tools": tools,
        "messages": req.messages.iter().map(message_json).collect::<Vec<_>>(),
    })
}

fn message_json(m: &ChatMessage) -> Value {
    let role = match m.role {
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    };
    let content: Vec<Value> = m
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult { tool_use_id, content, is_error } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
            ContentBlock::Image { media_type, data } => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data},
            }),
        })
        .collect();
    json!({"role": role, "content": content})
}

// ------------------------------------------------------------------ sse --

/// One server-sent event, already framed.
#[derive(Debug, PartialEq)]
struct Frame {
    event: String,
    data: String,
}

/// Incremental SSE framing: bytes in, complete events out. Handles events
/// split anywhere across chunk boundaries.
#[derive(Default)]
struct SseFrames {
    buf: String,
}

impl SseFrames {
    fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.push_str(&String::from_utf8_lossy(chunk));
        let mut frames = Vec::new();
        while let Some(end) = self.buf.find("\n\n") {
            let raw: String = self.buf.drain(..end + 2).collect();
            let mut event = String::new();
            let mut data = String::new();
            for line in raw.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(v.trim_start());
                }
            }
            if !event.is_empty() || !data.is_empty() {
                frames.push(Frame { event, data });
            }
        }
        frames
    }
}

// ------------------------------------------------------------- assembly --

/// Builds a [`ModelTurn`] out of streaming events. Returns text deltas to
/// forward as they arrive; tool-use inputs accumulate as partial JSON and
/// parse at block close.
#[derive(Default)]
struct Assembler {
    blocks: Vec<Partial>,
    stop: Option<StopReason>,
}

enum Partial {
    Text(String),
    ToolUse { id: String, name: String, input_json: String },
}

impl Assembler {
    fn handle(&mut self, frame: &Frame) -> Result<Option<String>> {
        let bad = |what: &str| AssistantError::Api(format!("malformed stream: {what}"));
        let data: Value = if frame.data.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&frame.data)
                .map_err(|e| bad(&format!("{e} in {:?}", frame.data)))?
        };
        match frame.event.as_str() {
            "content_block_start" => {
                let block = &data["content_block"];
                match block["type"].as_str() {
                    Some("text") => self.blocks.push(Partial::Text(
                        block["text"].as_str().unwrap_or_default().to_string(),
                    )),
                    Some("tool_use") => self.blocks.push(Partial::ToolUse {
                        id: block["id"].as_str().ok_or_else(|| bad("tool_use without id"))?.to_string(),
                        name: block["name"]
                            .as_str()
                            .ok_or_else(|| bad("tool_use without name"))?
                            .to_string(),
                        input_json: String::new(),
                    }),
                    other => {
                        return Err(bad(&format!("unsupported content block {other:?}")));
                    }
                }
                Ok(None)
            }
            "content_block_delta" => {
                let last = self.blocks.last_mut().ok_or_else(|| bad("delta before start"))?;
                match (last, data["delta"]["type"].as_str()) {
                    (Partial::Text(text), Some("text_delta")) => {
                        let piece =
                            data["delta"]["text"].as_str().unwrap_or_default().to_string();
                        text.push_str(&piece);
                        Ok((!piece.is_empty()).then_some(piece))
                    }
                    (Partial::ToolUse { input_json, .. }, Some("input_json_delta")) => {
                        input_json
                            .push_str(data["delta"]["partial_json"].as_str().unwrap_or_default());
                        Ok(None)
                    }
                    (_, other) => Err(bad(&format!("delta {other:?} for current block"))),
                }
            }
            "message_delta" => {
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    self.stop = Some(match reason {
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::MaxTokens,
                        _ => StopReason::EndTurn,
                    });
                }
                Ok(None)
            }
            "error" => Err(AssistantError::Api(format!(
                "stream error: {}",
                data["error"]["message"].as_str().unwrap_or("unknown")
            ))),
            // message_start, content_block_stop, message_stop, ping: nothing
            // to accumulate.
            _ => Ok(None),
        }
    }

    fn finish(self) -> Result<ModelTurn> {
        let content = self
            .blocks
            .into_iter()
            .map(|p| match p {
                Partial::Text(text) => Ok(ContentBlock::Text { text }),
                Partial::ToolUse { id, name, input_json } => {
                    let input = if input_json.is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&input_json).map_err(|e| {
                            AssistantError::Api(format!("tool input didn't parse: {e}"))
                        })?
                    };
                    Ok(ContentBlock::ToolUse { id, name, input })
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let stop = self
            .stop
            .ok_or_else(|| AssistantError::Api("stream ended without a stop reason".into()))?;
        Ok(ModelTurn { content, stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seam::ToolDef;

    #[test]
    fn sse_frames_survive_arbitrary_chunk_boundaries() {
        let raw = "event: message_start\ndata: {\"a\":1}\n\n\
                   event: ping\ndata: {}\n\n\
                   event: done\ndata: {\"b\":\n\
                   data: 2}\n\n";
        // Feed one byte at a time — the cruellest chunking.
        let mut frames = SseFrames::default();
        let mut got = Vec::new();
        for b in raw.as_bytes() {
            got.extend(frames.push(&[*b]));
        }
        assert_eq!(
            got,
            vec![
                Frame { event: "message_start".into(), data: "{\"a\":1}".into() },
                Frame { event: "ping".into(), data: "{}".into() },
                Frame { event: "done".into(), data: "{\"b\":\n2}".into() },
            ],
        );
    }

    fn feed(assembler: &mut Assembler, event: &str, data: &str) -> Option<String> {
        assembler
            .handle(&Frame { event: event.into(), data: data.into() })
            .unwrap()
    }

    #[test]
    fn assembles_text_and_tool_use_with_split_json() {
        let mut a = Assembler::default();
        feed(&mut a, "message_start", "{}");
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":""}}"#);
        assert_eq!(
            feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"Che"}}"#),
            Some("Che".into()),
        );
        assert_eq!(
            feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"cking."}}"#),
            Some("cking.".into()),
        );
        feed(&mut a, "content_block_stop", "{}");
        feed(
            &mut a,
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"c1","name":"queue_add"}}"#,
        );
        feed(
            &mut a,
            "content_block_delta",
            r#"{"delta":{"type":"input_json_delta","partial_json":"{\"title\":"}}"#,
        );
        feed(
            &mut a,
            "content_block_delta",
            r#"{"delta":{"type":"input_json_delta","partial_json":"\"Dal\"}"}}"#,
        );
        feed(&mut a, "content_block_stop", "{}");
        feed(&mut a, "message_delta", r#"{"delta":{"stop_reason":"tool_use"}}"#);
        feed(&mut a, "message_stop", "");

        let turn = a.finish().unwrap();
        assert_eq!(turn.stop, StopReason::ToolUse);
        assert_eq!(
            turn.content,
            vec![
                ContentBlock::Text { text: "Checking.".into() },
                ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "queue_add".into(),
                    input: json!({"title": "Dal"}),
                },
            ],
        );
    }

    #[test]
    fn empty_tool_input_parses_as_empty_object() {
        let mut a = Assembler::default();
        feed(
            &mut a,
            "content_block_start",
            r#"{"content_block":{"type":"tool_use","id":"c1","name":"queue_status"}}"#,
        );
        feed(&mut a, "message_delta", r#"{"delta":{"stop_reason":"tool_use"}}"#);
        let turn = a.finish().unwrap();
        assert_eq!(
            turn.content,
            vec![ContentBlock::ToolUse {
                id: "c1".into(),
                name: "queue_status".into(),
                input: json!({}),
            }],
        );
    }

    #[test]
    fn stream_error_and_truncation_surface() {
        let mut a = Assembler::default();
        let e = a
            .handle(&Frame {
                event: "error".into(),
                data: r#"{"error":{"message":"overloaded"}}"#.into(),
            })
            .unwrap_err();
        assert!(e.to_string().contains("overloaded"), "{e}");

        let a = Assembler::default();
        let e = a.finish().unwrap_err();
        assert!(e.to_string().contains("without a stop reason"), "{e}");
    }

    #[test]
    fn request_body_maps_the_seam_and_marks_the_cache_prefix() {
        let req = TurnRequest {
            system: "be helpful".into(),
            messages: vec![
                ChatMessage::user_text("hi"),
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "queue_status".into(),
                        input: json!({}),
                    }],
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: vec![
                        ContentBlock::ToolResult {
                            tool_use_id: "c1".into(),
                            content: "empty".into(),
                            is_error: false,
                        },
                        ContentBlock::Image {
                            media_type: "image/jpeg".into(),
                            data: "QUJD".into(),
                        },
                    ],
                },
            ],
            tools: vec![
                ToolDef { name: "a", description: "first", input_schema: json!({"type": "object"}) },
                ToolDef { name: "b", description: "last", input_schema: json!({"type": "object"}) },
            ],
        };
        let body = request_body("claude-opus-5", &req);

        assert_eq!(body["model"], "claude-opus-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["is_error"], false);
        assert_eq!(body["messages"][2]["content"][1]["source"]["media_type"], "image/jpeg");
    }
}
