//! Photo recon: propose-then-tap. A shelf photo rides one exchange as an
//! image block; the model answers with `propose_pantry_diff`, which the
//! drivers intercept like `fetch_url` — validated here, forwarded to the
//! UI as tappable lines, and *never* applied to the store. Misreads are
//! the expected case: every accepted line is one ordinary `pantry-set`
//! tap, and corrections are plain words on the same thread, which outrank
//! the photo. The photo itself is transient — the stored transcript keeps
//! a placeholder, the applied taps are what endure.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::seam::ContentBlock;
use crate::turn::{ToolCall, ToolOutcome};

/// The tool drivers must intercept and route here instead of the store.
pub const PROPOSE_PANTRY_DIFF: &str = "propose_pantry_diff";

/// What the Messages API accepts as an image source.
const MEDIA_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];

/// The Messages API refuses images above 5 MB decoded; this is that
/// ceiling in base64 chars (3 bytes → 4 chars), so anything the API
/// would bounce gets the friendly "downscale before upload" error
/// locally first. The web client downscales to well under this; the cap
/// is a backstop against raw phone photos entering through the CLI.
const MAX_DATA: usize = 5 * 1024 * 1024 / 3 * 4;

/// The presence vocabulary a proposal line may use — mirrors `pantry_set`.
const PRESENCES: &[&str] = &["have", "low", "out"];

/// Proposals larger than this are noise, not recon.
const MAX_LINES: usize = 80;

/// A recon rarely fits in one frame; it should still fit in one exchange.
const MAX_PHOTOS: usize = 12;

/// Combined base64 cap across an exchange's photos — the API's request
/// ceiling is the real limit, this keeps us clear of it. Pub because it
/// is the authoritative frame budget: the server derives its transport
/// limit from this number, so the friendly errors here stay reachable.
pub const MAX_TOTAL: usize = 20_000_000;

/// A photo attached to one user turn. Lives only in the live exchange:
/// never stored, never synced, never exported.
#[derive(Clone, Debug)]
pub struct Photo {
    /// e.g. `image/jpeg`
    pub media_type: String,
    /// Base64 payload.
    pub data: String,
}

impl Photo {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !MEDIA_TYPES.contains(&self.media_type.as_str()) {
            return Err(format!("unsupported image type {:?}", self.media_type));
        }
        if self.data.is_empty() {
            return Err("empty image".into());
        }
        if self.data.len() > MAX_DATA {
            return Err(format!(
                "image too large ({} bytes base64; downscale before upload)",
                self.data.len()
            ));
        }
        Ok(())
    }

    pub fn block(&self) -> ContentBlock {
        ContentBlock::Image { media_type: self.media_type.clone(), data: self.data.clone() }
    }
}

/// Validate a whole exchange's photos: each one, plus count and combined
/// size — one recon, several frames, still one request.
pub fn validate_all(photos: &[Photo]) -> std::result::Result<(), String> {
    if photos.len() > MAX_PHOTOS {
        return Err(format!("too many photos ({}; max {MAX_PHOTOS})", photos.len()));
    }
    let total: usize = photos.iter().map(|p| p.data.len()).sum();
    if total > MAX_TOTAL {
        return Err(format!("photos too large together ({total} bytes base64)"));
    }
    photos.iter().try_for_each(Photo::validate)
}

/// The transcript text stored for a user turn that carried photos. The
/// pixels are gone by the next exchange; the placeholder keeps the
/// transcript honest about what happened.
pub fn transcript_text(message: &str, photos: usize) -> String {
    let message = message.trim();
    let note = match photos {
        0 => return message.to_string(),
        1 => "[photo attached]".to_string(),
        n => format!("[{n} photos attached]"),
    };
    if message.is_empty() { note } else { format!("{message}\n\n{note}") }
}

/// A validated recon proposal, on its way to the UI. Serialized as-is into
/// the `proposal` SSE event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Location slug; the UI falls back to the active location if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub lines: Vec<ProposalLine>,
}

/// One tappable line — exactly the payload of one `pantry-set` tap, plus
/// the model's evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposalLine {
    /// Pantry item slug.
    pub item: String,
    /// have / low / out.
    pub presence: String,
    /// Display name for items not yet on the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What in the photo says so.
    pub reason: String,
}

/// Validate a raw `propose_pantry_diff` input. Errors are model-facing:
/// they land in an error tool result and the model retries.
pub fn parse_proposal(input: &Value) -> std::result::Result<Proposal, String> {
    let mut proposal: Proposal =
        serde_json::from_value(input.clone()).map_err(|e| format!("bad proposal: {e}"))?;
    if proposal.lines.is_empty() {
        return Err("a proposal needs at least one line; if the shelf matches the page, \
                    say so in your reply instead"
            .into());
    }
    if proposal.lines.len() > MAX_LINES {
        return Err(format!("too many lines ({}); propose the real differences", proposal.lines.len()));
    }
    if let Some(l) = &proposal.location {
        mise_core::types::Slug::new(l.trim()).map_err(|e| e.to_string())?;
        proposal.location = Some(l.trim().to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    for line in &mut proposal.lines {
        let slug = crate::tools::slugify(&line.item);
        if slug.is_empty() {
            return Err(format!("bad item {:?}", line.item));
        }
        line.item = slug;
        if !seen.insert(line.item.clone()) {
            return Err(format!("duplicate item {:?} — one line per item", line.item));
        }
        if !PRESENCES.contains(&line.presence.trim()) {
            return Err(format!(
                "bad presence {:?} for {:?} (want have/low/out)",
                line.presence, line.item
            ));
        }
        line.presence = line.presence.trim().to_string();
        line.reason = line.reason.trim().to_string();
        if line.reason.is_empty() {
            return Err(format!("line {:?} needs a reason — what in the photo says so?", line.item));
        }
        if let Some(name) = &line.name {
            let name = name.trim();
            line.name = (!name.is_empty()).then(|| name.to_string());
        }
    }
    Ok(proposal)
}

/// Run one `propose_pantry_diff` call: validate, hand the proposal to the
/// driver for display. Nothing here touches the store — the outcome tells
/// the model what happens next.
pub fn execute_propose(call: &ToolCall) -> (ToolOutcome, Option<Proposal>) {
    match parse_proposal(&call.input) {
        Ok(p) => {
            let outcome = ToolOutcome {
                tool_use_id: call.id.clone(),
                content: format!(
                    "Proposal shown to the user as {} tappable lines. They apply what's \
                     right and answer corrections in words — their words are ground \
                     truth. Nothing is changed until they tap. Briefly summarize the \
                     proposal in your reply; the photo is not kept.",
                    p.lines.len()
                ),
                is_error: false,
            };
            (outcome, Some(p))
        }
        Err(e) => {
            (ToolOutcome { tool_use_id: call.id.clone(), content: e, is_error: true }, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_photo_the_api_would_refuse_is_refused_here_first() {
        // Between our cap and the Messages API's 5 MB decoded ceiling
        // there must be nothing: anything the API would bounce gets the
        // friendly "downscale before upload" message locally instead.
        let photo = Photo { media_type: "image/jpeg".into(), data: "A".repeat(7_000_000) };
        let e = photo.validate().unwrap_err();
        assert!(e.contains("downscale before upload"), "{e}");
    }

    #[test]
    fn proposals_normalize_and_validate() {
        let p = parse_proposal(&json!({
            "location": " home ",
            "lines": [
                {"item": "Silken Tofu", "presence": "have", "reason": "two packs, front"},
                {"item": "miso", "presence": " out ", "name": "  ", "reason": "no jar visible"},
            ],
        }))
        .unwrap();
        assert_eq!(p.location.as_deref(), Some("home"));
        assert_eq!(p.lines[0].item, "silken-tofu");
        assert_eq!(p.lines[1].presence, "out");
        assert_eq!(p.lines[1].name, None, "blank names normalize away");
    }

    #[test]
    fn bad_proposals_come_back_as_model_facing_errors() {
        let cases = [
            (json!({"lines": []}), "at least one line"),
            (json!({"lines": [{"item": "ميزو", "presence": "out", "reason": "x"}]}), "bad item"),
            (
                json!({"lines": [
                    {"item": "miso", "presence": "out", "reason": "x"},
                    {"item": "Miso", "presence": "have", "reason": "y"},
                ]}),
                "duplicate item",
            ),
            (
                json!({"lines": [{"item": "miso", "presence": "gone", "reason": "x"}]}),
                "bad presence",
            ),
            (json!({"lines": [{"item": "miso", "presence": "out", "reason": "  "}]}), "reason"),
            (json!({"location": "no spaces", "lines": [{"item": "miso", "presence": "out", "reason": "x"}]}), ""),
            (json!({"nope": true}), "bad proposal"),
        ];
        for (input, needle) in cases {
            let e = parse_proposal(&input).unwrap_err();
            assert!(e.contains(needle), "{input} → {e}");
        }
    }

    #[test]
    fn oversized_proposals_are_refused() {
        let lines: Vec<Value> = (0..=MAX_LINES)
            .map(|i| json!({"item": format!("item-{i}"), "presence": "have", "reason": "seen"}))
            .collect();
        let e = parse_proposal(&json!({ "lines": lines })).unwrap_err();
        assert!(e.contains("too many lines"), "{e}");
    }

    #[test]
    fn execute_propose_never_needs_a_store() {
        use crate::turn::ToolCall;
        let call = ToolCall {
            id: "p1".into(),
            name: PROPOSE_PANTRY_DIFF.into(),
            input: json!({"lines": [{"item": "rice", "presence": "have", "reason": "big bag"}]}),
        };
        let (outcome, proposal) = execute_propose(&call);
        assert!(!outcome.is_error);
        assert!(outcome.content.contains("1 tappable"), "{}", outcome.content);
        assert_eq!(proposal.unwrap().lines[0].item, "rice");

        let bad = ToolCall { id: "p2".into(), name: PROPOSE_PANTRY_DIFF.into(), input: json!({}) };
        let (outcome, proposal) = execute_propose(&bad);
        assert!(outcome.is_error);
        assert!(proposal.is_none());
    }

    #[test]
    fn photos_validate_type_and_size() {
        let ok = Photo { media_type: "image/jpeg".into(), data: "QUJD".into() };
        assert!(ok.validate().is_ok());
        let bad_type = Photo { media_type: "image/tiff".into(), data: "QUJD".into() };
        assert!(bad_type.validate().unwrap_err().contains("unsupported"));
        let empty = Photo { media_type: "image/png".into(), data: String::new() };
        assert!(empty.validate().unwrap_err().contains("empty"));
        let huge = Photo { media_type: "image/png".into(), data: "A".repeat(MAX_DATA + 1) };
        assert!(huge.validate().unwrap_err().contains("too large"));
    }

    #[test]
    fn transcript_placeholder_keeps_the_export_honest() {
        assert_eq!(transcript_text("hi", 0), "hi");
        assert_eq!(transcript_text("  ", 1), "[photo attached]");
        assert_eq!(transcript_text("pantry, left side", 1), "pantry, left side\n\n[photo attached]");
        assert_eq!(transcript_text("the whole pantry", 3), "the whole pantry\n\n[3 photos attached]");
    }

    #[test]
    fn photo_batches_validate_count_and_combined_size() {
        let small = |n: usize| Photo { media_type: "image/jpeg".into(), data: "A".repeat(n) };
        assert!(validate_all(&[small(100), small(100)]).is_ok());
        let crowd: Vec<Photo> = (0..MAX_PHOTOS + 1).map(|_| small(10)).collect();
        assert!(validate_all(&crowd).unwrap_err().contains("too many photos"));
        let heavy: Vec<Photo> = (0..4).map(|_| small(6_000_000)).collect();
        assert!(validate_all(&heavy).unwrap_err().contains("together"));
        assert!(
            validate_all(&[Photo { media_type: "image/tiff".into(), data: "QQ==".into() }])
                .unwrap_err()
                .contains("unsupported"),
            "per-photo checks still apply"
        );
    }
}
