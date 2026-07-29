//! Local Remote Compaction V2 shim for non-native upstreams.
//!
//! Codex Desktop expects exactly one `{type:"compaction", encrypted_content:…}`
//! output item. Third-party / Chat Completions routes cannot mint OpenAI
//! ciphertext, so Spur runs the **current** route as a plain summarizer and
//! wraps the text in a portable envelope (`spur1:` + base64).
//!
//! Design notes (see product plan):
//! - Compact only with history the **current** model can still read (decode
//!   our own envelopes; never pretend to decrypt foreign `gAAAAA…` blobs).
//! - Cross-provider handoff-on-old is a later phase; this module only covers
//!   same-route compact so mid-thread Desktop remote compact does not fatal.
//! - Inspired by the OpenCodex compact contract (MIT); independent Rust code.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};

/// Portable compact envelope prefix (not OpenAI KMS ciphertext).
pub const SPUR_COMPACTION_PREFIX: &str = "spur1:";

/// Also accept OpenCodex envelopes if a thread mixed tools (decode-only).
pub const OCX_COMPACTION_PREFIX: &str = "ocx1:";

/// Official compact template fallback (codex-rs `prompts/templates/compact/prompt.md`).
/// Prefer [`effective_compact_prompt`] so `experimental_compact_prompt_file` maps in.
#[allow(dead_code)] // public API + unit tests; runtime uses effective_compact_prompt()
pub const COMPACT_PROMPT: &str = crate::official_prompt_map::OFFICIAL_COMPACT_PROMPT_FALLBACK;

/// Framing when a portable summary is replayed into a later turn.
/// Official `summary_prefix.md` — not Spur-authored.
pub const SUMMARY_PREFIX: &str = crate::official_prompt_map::OFFICIAL_SUMMARY_PREFIX_FALLBACK;

/// Compact prompt mapped from Codex home config, else official OSS template.
pub fn effective_compact_prompt() -> String {
    let mapped = crate::official_prompt_map::resolve_compact_prompt();
    if mapped.source != crate::official_prompt_map::CompactPromptSource::OfficialFallback {
        tracing::debug!(
            label = %mapped.source_label,
            chars = mapped.text.len(),
            "using mapped compact prompt override"
        );
    }
    mapped.text
}

pub const OPAQUE_COMPACTION_NOTE: &str =
    "[earlier conversation was compacted; the summary is stored in a format this model cannot read]";

/// Only official OpenAI kind uses upstream native Compact V2. Everything else
/// (Chat bridge, custom/xAI/MiniMax Responses) uses the local shim.
pub fn uses_native_remote_compaction(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("openai")
}

pub fn encode_spur_compaction_summary(summary: &str) -> String {
    format!("{SPUR_COMPACTION_PREFIX}{}", B64.encode(summary.as_bytes()))
}

/// Decode Spur or OpenCodex portable envelopes. Real OpenAI ciphertext → None.
pub fn decode_portable_compaction_summary(encrypted_content: &str) -> Option<String> {
    let (prefix, rest) = if let Some(rest) = encrypted_content.strip_prefix(SPUR_COMPACTION_PREFIX) {
        (SPUR_COMPACTION_PREFIX, rest)
    } else if let Some(rest) = encrypted_content.strip_prefix(OCX_COMPACTION_PREFIX) {
        (OCX_COMPACTION_PREFIX, rest)
    } else {
        return None;
    };
    let _ = prefix;
    B64.decode(rest.as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// User-visible text for a historical compaction item.
pub fn compaction_item_to_text(encrypted_content: Option<&str>) -> String {
    match encrypted_content.and_then(decode_portable_compaction_summary) {
        Some(summary) => format!("{SUMMARY_PREFIX}\n\n{summary}"),
        None => OPAQUE_COMPACTION_NOTE.to_string(),
    }
}

fn content_plain_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                let ty = part.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(ty, "input_text" | "output_text" | "text") {
                    part.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Build a plain transcript the **current** model can summarize.
///
/// - Decodes portable `spur1:` / `ocx1:` compaction items.
/// - Foreign encrypted compaction → short opaque note (no fake recovery).
/// - Skips the live compaction control carrier and pure reasoning ciphertext.
/// - Keeps messages + tool call trail as text.
pub fn portable_transcript_for_compact(request: &Value) -> String {
    let live = live_compaction_index(request);
    let Some(items) = request.get("input").and_then(Value::as_array) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if live == Some(index) && item_type == "compaction" {
            continue;
        }
        match item_type {
            "compaction" | "compaction_summary" | "context_compaction" => {
                let encrypted = item.get("encrypted_content").and_then(Value::as_str);
                lines.push(format!("[compaction]\n{}", compaction_item_to_text(encrypted)));
            }
            "reasoning" => {
                // Prefer plaintext summary if present; never invent from ciphertext.
                let summary = item
                    .get("summary")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                if !summary.trim().is_empty() {
                    lines.push(format!("[reasoning]\n{}", summary.trim()));
                }
            }
            "function_call" => {
                let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                let args = item
                    .get("arguments")
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                lines.push(format!("[tool_call {name}]\n{args}"));
            }
            "function_call_output" => {
                let out = match item.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                if !out.trim().is_empty() {
                    lines.push(format!("[tool_result]\n{}", out.trim()));
                }
            }
            "message" | "" => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let text = item
                    .get("content")
                    .map(content_plain_text)
                    .or_else(|| {
                        item.get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    lines.push(format!("[{role}]\n{}", text.trim()));
                }
            }
            "item_reference" | "additional_tools" => {}
            _ => {
                if item.get("encrypted_content").is_some() {
                    lines.push(format!("[opaque]\n{OPAQUE_COMPACTION_NOTE}"));
                }
            }
        }
    }
    lines.join("\n\n")
}

fn live_compaction_index(request: &Value) -> Option<usize> {
    request
        .get("input")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .rposition(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        })
}

/// Rough char budget for the summarizer input (~4 chars/token). Cap so a
/// 258k OpenAI thread is not dumped into a smaller model raw.
pub fn compact_input_char_budget(context_window_tokens: Option<i64>) -> usize {
    let tokens = context_window_tokens.unwrap_or(32_000).clamp(4_000, 200_000) as usize;
    // Leave headroom for the compact instruction and the summary response.
    let usable = tokens.saturating_mul(60) / 100;
    usable.saturating_mul(4).min(400_000)
}

pub fn trim_transcript_to_budget(transcript: &str, budget_chars: usize) -> String {
    if transcript.len() <= budget_chars {
        return transcript.to_string();
    }
    // Keep the tail (most recent context).
    let start = transcript.len().saturating_sub(budget_chars);
    let tail = &transcript[start..];
    format!("[…earlier transcript truncated for context budget…]\n\n{tail}")
}

pub fn build_compact_user_prompt(transcript: &str, context_window_tokens: Option<i64>) -> String {
    let budget = compact_input_char_budget(context_window_tokens);
    let body = trim_transcript_to_budget(transcript, budget);
    let compact_prompt = effective_compact_prompt();
    if body.trim().is_empty() {
        format!(
            "{compact_prompt}\n\n---\n\n(no portable conversation text was available; earlier history may have been encrypted under another provider)"
        )
    } else {
        format!("{compact_prompt}\n\n---\n\n{body}")
    }
}

/// Responses JSON body Desktop accepts for a completed remote compact turn.
pub fn synthetic_compaction_response(model: &str, summary: &str) -> Value {
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    let item_id = format!("cmp_{}", uuid::Uuid::new_v4().simple());
    json!({
        "id": response_id,
        "object": "response",
        "status": "completed",
        "model": model,
        "output": [{
            "type": "compaction",
            "id": item_id,
            "encrypted_content": encode_spur_compaction_summary(summary)
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": summary.len() / 4,
            "total_tokens": summary.len() / 4
        }
    })
}

/// Minimal SSE lifecycle for a compact turn (non-streaming Desktop still OK with JSON).
pub fn synthetic_compaction_sse(model: &str, summary: &str) -> String {
    let response = synthetic_compaction_response(model, summary);
    let response_id = response.get("id").and_then(Value::as_str).unwrap_or("resp_cmp");
    let item = response
        .pointer("/output/0")
        .cloned()
        .unwrap_or_else(|| json!({"type":"compaction"}));
    format!(
        "event: response.created\ndata: {}\n\n\
event: response.in_progress\ndata: {}\n\n\
event: response.output_item.done\ndata: {}\n\n\
event: response.completed\ndata: {}\n\n",
        json!({"type":"response.created","response":{"id":response_id,"status":"in_progress","model":model,"output":[]}}),
        json!({"type":"response.in_progress","response":{"id":response_id,"status":"in_progress","model":model,"output":[]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":response}),
    )
}

/// Expand historical portable compaction items into plain user messages for
/// outbound sanitizers. Foreign encrypted → opaque note message. Live control
/// carrier is left for native paths; shim intercepts before this runs.
/// (Proxy currently inlines equivalent logic; kept for shared call sites/tests.)
#[allow(dead_code)]
pub fn expand_historical_compaction_items(request: &mut Value, keep_live_carrier: bool) -> bool {
    let live = live_compaction_index(request);
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    let mut next = Vec::with_capacity(items.len());
    for (index, item) in items.drain(..).enumerate() {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        let is_compact_family = matches!(
            item_type,
            "compaction" | "compaction_summary" | "context_compaction"
        );
        if is_compact_family {
            if keep_live_carrier && live == Some(index) && item_type == "compaction" {
                next.push(item);
                continue;
            }
            let encrypted = item.get("encrypted_content").and_then(Value::as_str);
            // context_compaction without payload is a pure marker — drop.
            if item_type == "context_compaction" && encrypted.is_none() {
                changed = true;
                continue;
            }
            let text = compaction_item_to_text(encrypted);
            next.push(json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}]
            }));
            changed = true;
            continue;
        }
        next.push(item);
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("input".into(), Value::Array(next));
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_and_summary_constants_match_official_fallback() {
        assert_eq!(
            COMPACT_PROMPT,
            crate::official_prompt_map::OFFICIAL_COMPACT_PROMPT_FALLBACK
        );
        assert_eq!(
            SUMMARY_PREFIX,
            crate::official_prompt_map::OFFICIAL_SUMMARY_PREFIX_FALLBACK
        );
        let prompt = build_compact_user_prompt("hello transcript", None);
        assert!(prompt.starts_with(COMPACT_PROMPT));
        assert!(prompt.contains("hello transcript"));
    }

    #[test]
    fn spur_envelope_round_trips() {
        let encoded = encode_spur_compaction_summary("hello checkpoint");
        assert!(encoded.starts_with(SPUR_COMPACTION_PREFIX));
        assert_eq!(
            decode_portable_compaction_summary(&encoded).as_deref(),
            Some("hello checkpoint")
        );
    }

    #[test]
    fn foreign_ciphertext_does_not_decode() {
        assert!(decode_portable_compaction_summary("gAAAAA-not-ours").is_none());
        assert!(compaction_item_to_text(Some("gAAAAA-not-ours")).contains("cannot read"));
    }

    #[test]
    fn ocx_envelope_decodes_for_interop() {
        let raw = "from opencodex";
        let encoded = format!("{OCX_COMPACTION_PREFIX}{}", B64.encode(raw.as_bytes()));
        assert_eq!(
            decode_portable_compaction_summary(&encoded).as_deref(),
            Some(raw)
        );
    }

    #[test]
    fn portable_transcript_decodes_spur_and_notes_foreign() {
        let body = json!({
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
                {"type":"compaction","encrypted_content": encode_spur_compaction_summary("prior work")},
                {"type":"compaction","encrypted_content":"gAAAAA-secret"},
                {"type":"compaction"}
            ]
        });
        let text = portable_transcript_for_compact(&body);
        assert!(text.contains("hi"));
        assert!(text.contains("prior work"));
        assert!(text.contains("cannot read"));
        // Live carrier last is skipped.
        assert!(!text.contains("\"type\":\"compaction\"}"));
    }

    #[test]
    fn expand_historical_turns_spur_into_message_keeps_live() {
        let mut body = json!({
            "input": [
                {"type":"compaction","encrypted_content": encode_spur_compaction_summary("keep me")},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"go"}]},
                {"type":"compaction"}
            ]
        });
        assert!(expand_historical_compaction_items(&mut body, true));
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], "message");
        assert!(input[0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("keep me"));
        assert_eq!(input[2]["type"], "compaction");
        assert!(input[2].get("encrypted_content").is_none());
    }

    #[test]
    fn synthetic_response_has_exactly_one_compaction() {
        let body = synthetic_compaction_response("gpt-test", "summary text");
        let output = body["output"].as_array().unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["type"], "compaction");
        let enc = output[0]["encrypted_content"].as_str().unwrap();
        assert_eq!(
            decode_portable_compaction_summary(enc).as_deref(),
            Some("summary text")
        );
    }

    #[test]
    fn trim_keeps_tail() {
        let big = "a".repeat(100) + "TAIL";
        let trimmed = trim_transcript_to_budget(&big, 10);
        assert!(trimmed.ends_with("TAIL") || trimmed.contains("TAIL"));
        assert!(trimmed.len() < big.len());
    }
}
