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

/// A blackholed connection (NAT expiry, suspend, uplink flap) must fail,
/// not hang a stream open forever: bound the TCP connect and the silence
/// between chunks. `read_timeout` is per-read, so a long exchange is fine
/// as long as bytes keep arriving.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

#[derive(Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    pub model: String,
}

impl AnthropicClient {
    pub fn new(api_key: impl Into<String>) -> AnthropicClient {
        AnthropicClient {
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .tcp_keepalive(std::time::Duration::from_secs(30))
                .build()
                .expect("client construction is infallible with these options"),
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
        // Retries happen only here, before any delta has been emitted:
        // once streaming starts, a broken stream is a broken exchange.
        let mut attempt = 0;
        let response = loop {
            attempt += 1;
            let sent = self
                .http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await;
            let response = match sent {
                Ok(r) => r,
                Err(e) => match backoff(None, attempt, None) {
                    Some(delay) => {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    None => return Err(AssistantError::Api(format!("request failed: {e}"))),
                },
            };
            let status = response.status();
            if status.is_success() {
                break response;
            }
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            match backoff(Some(status.as_u16()), attempt, retry_after) {
                Some(delay) => tokio::time::sleep(delay).await,
                None => {
                    let detail = response.text().await.unwrap_or_default();
                    let message = serde_json::from_str::<Value>(&detail)
                        .ok()
                        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                        .unwrap_or(detail);
                    return Err(AssistantError::Api(format!("{status}: {message}")));
                }
            }
        };

        let mut frames = SseFrames::default();
        let mut assembler = Assembler::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| AssistantError::Api(format!("stream broke: {e}")))?;
            for frame in frames.push(&chunk)? {
                if let Some(delta) = assembler.handle(&frame)? {
                    on_delta(&delta);
                }
            }
        }
        for frame in frames.finish() {
            if let Some(delta) = assembler.handle(&frame)? {
                on_delta(&delta);
            }
        }
        assembler.finish()
    }
}

/// Attempts before giving up, counting the first.
const MAX_ATTEMPTS: usize = 3;

/// The retry decision, pure so it is testable without a network: whether
/// this failure warrants another attempt and after how long. `status` is
/// `None` for a transport failure (connect refused, reset before headers)
/// — those never emitted a delta either, so they are equally safe to
/// retry. 4xx other than 429 means the request itself is wrong; retrying
/// would re-send the same mistake.
fn backoff(
    status: Option<u16>,
    attempt: usize,
    retry_after_secs: Option<u64>,
) -> Option<std::time::Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let retryable = match status {
        None => true,
        Some(429) => true,
        Some(s) => (500..=599).contains(&s), // 529 overloaded included
    };
    if !retryable {
        return None;
    }
    // Honour retry-after within reason; otherwise exponential from 1 s.
    let secs = retry_after_secs.unwrap_or(1 << (attempt - 1)).min(30);
    Some(std::time::Duration::from_secs(secs))
}

// ------------------------------------------------------------- requests --

/// Our seam types → the Messages API body. Three `cache_control` markers:
/// the system prompt, the last tool, and the tail of the message list.
/// The first two cover the stable prefix; the third is re-planted on every
/// request because *within* an exchange the tail is append-only — each of
/// up to MAX_TOOL_ROUNDS re-sends every prior turn and tool result, which
/// is exactly what is worth caching.
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
    let mut messages: Vec<Value> = req.messages.iter().map(message_json).collect();
    if let Some(block) = messages
        .last_mut()
        .and_then(|m| m["content"].as_array_mut())
        .and_then(|blocks| blocks.last_mut())
    {
        block["cache_control"] = json!({"type": "ephemeral"});
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
        "messages": messages,
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
            ContentBlock::Thinking { thinking, signature } => {
                json!({"type": "thinking", "thinking": thinking, "signature": signature})
            }
            ContentBlock::RedactedThinking { data } => {
                json!({"type": "redacted_thinking", "data": data})
            }
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

/// A stream that never yields a complete line cannot grow the buffer
/// forever; well past any real event, the stream is declared broken.
const MAX_SSE_BUF: usize = 16 * 1024 * 1024;

/// Incremental SSE framing: bytes in, complete events out. Byte-oriented —
/// chunks split mid-character stay bytes until a full line arrives, so
/// multi-byte text survives any chunk boundary — and line-ending-agnostic
/// (LF, CRLF, lone CR), since the client is explicitly designed to be
/// pointed at proxies and fakes.
#[derive(Default)]
struct SseFrames {
    buf: Vec<u8>,
    event: String,
    data: String,
}

impl SseFrames {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Frame>> {
        self.buf.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(line) = self.next_line() {
            if line.is_empty() {
                // Blank line: the event is complete.
                if !self.event.is_empty() || !self.data.is_empty() {
                    frames.push(Frame {
                        event: std::mem::take(&mut self.event),
                        data: std::mem::take(&mut self.data),
                    });
                }
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(v.trim_start());
            }
        }
        if self.buf.len() > MAX_SSE_BUF {
            return Err(AssistantError::Api("SSE line overran the buffer cap".into()));
        }
        Ok(frames)
    }

    /// The stream is over: a held trailing `\r` was a lone terminator
    /// after all, so resolve it and drain what completes. Anything still
    /// unterminated — a partial line, an event with no closing blank
    /// line — is discarded, as the SSE spec says it must be.
    fn finish(&mut self) -> Vec<Frame> {
        if self.buf.last() == Some(&b'\r') {
            *self.buf.last_mut().expect("just checked") = b'\n';
        }
        let mut frames = Vec::new();
        while let Some(line) = self.next_line() {
            if line.is_empty() && (!self.event.is_empty() || !self.data.is_empty()) {
                frames.push(Frame {
                    event: std::mem::take(&mut self.event),
                    data: std::mem::take(&mut self.data),
                });
            }
        }
        frames
    }

    /// Pop one complete line off the buffer, if one has fully arrived.
    /// Line terminators are ASCII, so a complete line is always complete
    /// UTF-8. A `\r` as the final buffered byte is ambiguous — the `\n`
    /// of a CRLF may still be in flight — so it waits for the next chunk.
    fn next_line(&mut self) -> Option<String> {
        let end = self.buf.iter().position(|b| matches!(b, b'\n' | b'\r'))?;
        let skip = match self.buf[end] {
            b'\r' if end + 1 == self.buf.len() => return None,
            b'\r' if self.buf[end + 1] == b'\n' => 2,
            _ => 1,
        };
        let line = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        self.buf.drain(..end + skip);
        Some(line)
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
    Thinking { thinking: String, signature: String },
    Redacted { data: String },
    /// A block type this build doesn't know. Kept as a placeholder so block
    /// indices stay aligned, dropped at finish. One policy for all unknowns:
    /// skip and warn, like unknown SSE events — API drift must not take down
    /// live exchanges. Hard errors are reserved for structurally broken data.
    Unknown,
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
                    Some("thinking") => self.blocks.push(Partial::Thinking {
                        thinking: block["thinking"].as_str().unwrap_or_default().to_string(),
                        signature: String::new(),
                    }),
                    Some("redacted_thinking") => self.blocks.push(Partial::Redacted {
                        data: block["data"].as_str().unwrap_or_default().to_string(),
                    }),
                    other => {
                        eprintln!("mise: skipping unknown content block {other:?} from the model API");
                        self.blocks.push(Partial::Unknown);
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
                    (Partial::Thinking { thinking, .. }, Some("thinking_delta")) => {
                        thinking
                            .push_str(data["delta"]["thinking"].as_str().unwrap_or_default());
                        Ok(None)
                    }
                    (Partial::Thinking { signature, .. }, Some("signature_delta")) => {
                        signature
                            .push_str(data["delta"]["signature"].as_str().unwrap_or_default());
                        Ok(None)
                    }
                    // Deltas addressed to a block we're skipping are skipped
                    // with it, whatever their type.
                    (Partial::Unknown, _) => Ok(None),
                    // A *known* delta kind on the wrong block is structurally
                    // broken data — the stream is lying about its own shape.
                    (
                        _,
                        other @ Some(
                            "text_delta" | "input_json_delta" | "thinking_delta"
                            | "signature_delta",
                        ),
                    ) => Err(bad(&format!("delta {other:?} for current block"))),
                    // An unknown delta kind (a future citations_delta, say)
                    // degrades like an unknown SSE event: skip and warn.
                    (_, other) => {
                        eprintln!("mise: ignoring unknown delta {other:?} from the model API");
                        Ok(None)
                    }
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
        let stop = self
            .stop
            .ok_or_else(|| AssistantError::Api("stream ended without a stop reason".into()))?;
        let mut content = Vec::new();
        for p in self.blocks {
            content.push(match p {
                Partial::Text(text) => ContentBlock::Text { text },
                Partial::ToolUse { id, name, input_json } => {
                    let input = if input_json.is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str(&input_json) {
                            Ok(v) => v,
                            // A max_tokens cut mid input_json leaves a JSON
                            // prefix. The turn driver's contract is "yield
                            // what text we have"; drop the half-call so it
                            // can, instead of failing the whole turn.
                            Err(_) if stop == StopReason::MaxTokens => continue,
                            Err(e) => {
                                return Err(AssistantError::Api(format!(
                                    "tool input didn't parse: {e}"
                                )));
                            }
                        }
                    };
                    ContentBlock::ToolUse { id, name, input }
                }
                Partial::Thinking { thinking, signature } => {
                    ContentBlock::Thinking { thinking, signature }
                }
                Partial::Redacted { data } => ContentBlock::RedactedThinking { data },
                Partial::Unknown => continue,
            });
        }
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
            got.extend(frames.push(&[*b]).unwrap());
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

    #[test]
    fn sse_frames_refuse_a_stream_with_no_line_endings() {
        let mut frames = SseFrames::default();
        let chunk = vec![b'x'; 1024 * 1024];
        let e = (0..17)
            .find_map(|_| frames.push(&chunk).err())
            .expect("an endless line must not grow the buffer forever");
        assert!(e.to_string().contains("buffer cap"), "{e}");
    }

    #[test]
    fn sse_frames_never_tear_multibyte_characters() {
        // Real corpus text: accents, the project's own em dashes, CJK.
        let payload = r#"{"text":"sauté — crème fraîche, æøå, 麻婆豆腐"}"#;
        let raw = format!("event: content_block_delta\ndata: {payload}\n\n");
        let mut frames = SseFrames::default();
        let mut got = Vec::new();
        // One byte at a time guarantees every multi-byte character is
        // split across a chunk boundary somewhere.
        for b in raw.as_bytes() {
            got.extend(frames.push(&[*b]).unwrap());
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, payload, "no replacement characters, no dropped bytes");
    }

    #[test]
    fn sse_frames_accept_any_line_ending() {
        // The client is explicitly designed to be pointed at proxies and
        // fakes; SSE permits CRLF, LF, and lone-CR line endings.
        for ending in ["\n", "\r\n", "\r"] {
            let raw = format!(
                "event: message_start{ending}data: {{\"a\":1}}{ending}{ending}\
                 event: done{ending}data: {{\"b\":{ending}data: 2}}{ending}{ending}"
            );
            let mut frames = SseFrames::default();
            let mut got = Vec::new();
            for chunk in raw.as_bytes().chunks(3) {
                got.extend(frames.push(chunk).unwrap());
            }
            got.extend(frames.finish());
            assert_eq!(
                got,
                vec![
                    Frame { event: "message_start".into(), data: "{\"a\":1}".into() },
                    Frame { event: "done".into(), data: "{\"b\":\n2}".into() },
                ],
                "with line ending {ending:?}",
            );
        }
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
    fn thinking_blocks_assemble_opaquely_and_round_trip() {
        let mut a = Assembler::default();
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"thinking","thinking":""}}"#);
        assert_eq!(
            feed(&mut a, "content_block_delta", r#"{"delta":{"type":"thinking_delta","thinking":"hmm, curry twice"}}"#),
            None,
            "reasoning never streams as visible text",
        );
        feed(&mut a, "content_block_delta", r#"{"delta":{"type":"signature_delta","signature":"sig123"}}"#);
        feed(&mut a, "content_block_stop", "{}");
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"redacted_thinking","data":"opaque=="}}"#);
        feed(&mut a, "content_block_stop", "{}");
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":""}}"#);
        feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"Dal it is."}}"#);
        feed(&mut a, "message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#);

        let turn = a.finish().unwrap();
        assert_eq!(
            turn.content,
            vec![
                ContentBlock::Thinking {
                    thinking: "hmm, curry twice".into(),
                    signature: "sig123".into(),
                },
                ContentBlock::RedactedThinking { data: "opaque==".into() },
                ContentBlock::Text { text: "Dal it is.".into() },
            ],
        );

        // …and both map back to the wire verbatim, signature included —
        // the API checks it when a tool loop continues.
        let msg = message_json(&ChatMessage { role: ChatRole::Assistant, content: turn.content });
        assert_eq!(msg["content"][0]["type"], "thinking");
        assert_eq!(msg["content"][0]["signature"], "sig123");
        assert_eq!(msg["content"][1]["type"], "redacted_thinking");
        assert_eq!(msg["content"][1]["data"], "opaque==");
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

    /// A `max_tokens` cut mid `input_json_delta` leaves a JSON prefix in
    /// the tool block. The turn driver's contract is "yield what text we
    /// have" — so the half-call is dropped and the text survives, instead
    /// of the whole turn failing as a parse error.
    #[test]
    fn a_truncated_tool_call_yields_the_text_that_preceded_it() {
        let truncated = |stop: &str| {
            let mut a = Assembler::default();
            feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":"Adding it"}}"#);
            feed(&mut a, "content_block_stop", "{}");
            feed(
                &mut a,
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"c1","name":"queue_add"}}"#,
            );
            feed(
                &mut a,
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"title\":\"Da"}}"#,
            );
            feed(&mut a, "message_delta", &format!(r#"{{"delta":{{"stop_reason":"{stop}"}}}}"#));
            a.finish()
        };

        let turn = truncated("max_tokens").unwrap();
        assert_eq!(turn.stop, StopReason::MaxTokens);
        assert_eq!(turn.content, vec![ContentBlock::Text { text: "Adding it".into() }]);

        // Under any other stop reason a broken tool input is still fatal:
        // the model claims the call is complete and it does not parse.
        let e = truncated("tool_use").unwrap_err();
        assert!(e.to_string().contains("tool input didn't parse"), "{e}");
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

    /// The retry policy in one table: what retries, what doesn't, and for
    /// how long. The loop around it is the thin IO remainder, exercised by
    /// evals and real use per the charter.
    #[test]
    fn retries_are_bounded_backed_off_and_honour_retry_after() {
        use std::time::Duration;

        // Transport failures and 429/5xx retry, with exponential backoff.
        assert_eq!(backoff(None, 1, None), Some(Duration::from_secs(1)));
        assert_eq!(backoff(Some(429), 1, None), Some(Duration::from_secs(1)));
        assert_eq!(backoff(Some(529), 2, None), Some(Duration::from_secs(2)));
        assert_eq!(backoff(Some(500), 2, None), Some(Duration::from_secs(2)));

        // retry-after is honoured, but never past the cap.
        assert_eq!(backoff(Some(429), 1, Some(7)), Some(Duration::from_secs(7)));
        assert_eq!(backoff(Some(429), 1, Some(3600)), Some(Duration::from_secs(30)));

        // The budget is bounded: the last attempt's failure is final.
        assert_eq!(backoff(Some(429), MAX_ATTEMPTS, None), None);
        assert_eq!(backoff(None, MAX_ATTEMPTS, None), None);

        // A request the server rejected as wrong stays rejected.
        assert_eq!(backoff(Some(400), 1, None), None);
        assert_eq!(backoff(Some(401), 1, None), None);
        assert_eq!(backoff(Some(404), 1, None), None);
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

        // Within an exchange the tail is append-only — every tool round
        // re-sends the whole conversation — so the last block of the last
        // message is the third breakpoint. Only that one: earlier blocks
        // ride inside the prefix it closes.
        assert_eq!(body["messages"][2]["content"][1]["cache_control"]["type"], "ephemeral");
        assert!(body["messages"][2]["content"][0].get("cache_control").is_none());
        assert!(body["messages"][0]["content"][0].get("cache_control").is_none());
    }

    /// A future block type (say, a citations container) must degrade like an
    /// unknown SSE event does — skipped, indices aligned — not take down a
    /// live user exchange. Hard errors stay reserved for structurally broken
    /// data, tested below.
    #[test]
    fn an_unknown_block_type_is_skipped_not_fatal() {
        let mut a = Assembler::default();
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"citations_box"}}"#);
        // Deltas addressed to the skipped block are skipped with it,
        // whatever their type.
        feed(&mut a, "content_block_delta", r#"{"delta":{"type":"citations_delta","c":"x"}}"#);
        feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"lost"}}"#);
        feed(&mut a, "content_block_stop", "{}");
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":""}}"#);
        feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"Dal."}}"#);
        feed(&mut a, "message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#);

        let turn = a.finish().unwrap();
        assert_eq!(turn.content, vec![ContentBlock::Text { text: "Dal.".into() }]);
    }

    #[test]
    fn an_unknown_delta_kind_on_a_known_block_is_ignored() {
        let mut a = Assembler::default();
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":""}}"#);
        assert_eq!(
            feed(&mut a, "content_block_delta", r#"{"delta":{"type":"citations_delta","c":"x"}}"#),
            None,
        );
        assert_eq!(
            feed(&mut a, "content_block_delta", r#"{"delta":{"type":"text_delta","text":"Dal."}}"#),
            Some("Dal.".into()),
            "the block keeps accumulating after the ignored delta",
        );
        feed(&mut a, "message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#);
        let turn = a.finish().unwrap();
        assert_eq!(turn.content, vec![ContentBlock::Text { text: "Dal.".into() }]);
    }

    /// The other half of the policy: a *known* delta kind addressed to the
    /// wrong block is structurally broken data, and stays fatal.
    #[test]
    fn a_known_delta_on_the_wrong_block_is_still_fatal() {
        let mut a = Assembler::default();
        feed(&mut a, "content_block_start", r#"{"content_block":{"type":"text","text":""}}"#);
        let e = a
            .handle(&Frame {
                event: "content_block_delta".into(),
                data: r#"{"delta":{"type":"input_json_delta","partial_json":"{"}}"#.into(),
            })
            .unwrap_err();
        assert!(e.to_string().contains("delta"), "{e}");
    }
}
