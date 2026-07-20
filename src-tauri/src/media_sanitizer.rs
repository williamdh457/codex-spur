//! Replace image blocks with text markers for text-only upstream models.
//!
//! Behavioral reference: CC Switch media sanitizer (MIT) — independent implementation.
//! Desktop may attach `input_image` / vision parts that DeepSeek and some CN gateways
//! hard-reject with 400 "only support text".

use serde_json::{json, Value};

/// Marker shown in the transcript when an image was stripped before upstream send.
pub const UNSUPPORTED_IMAGE_MARKER: &str = "[image omitted: this model only accepts text]";

/// Kinds that default to text-only (no vision) unless catalog later overrides.
pub fn kind_is_text_only_default(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "deepseek" | "minimax"
    )
}

/// Model id heuristics for text-only (last path segment).
pub fn model_looks_text_only(model: &str) -> bool {
    let model = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    if model.contains("vision") || model.contains("vl") || model.contains("image") {
        return false;
    }
    // Known text-only families / brands.
    model.starts_with("deepseek")
        || model.starts_with("minimax")
        || model.contains("longcat")
        || model.starts_with("mimo")
        || model.contains("coder") && model.contains("qwen")
}

pub fn should_strip_images(kind: &str, upstream_model: &str) -> bool {
    if kind.eq_ignore_ascii_case("openai") || kind.eq_ignore_ascii_case("xai") {
        return false;
    }
    if kind.eq_ignore_ascii_case("kimi") {
        // Kimi coding path supports multimodal for some models; keep images by default.
        return false;
    }
    kind_is_text_only_default(kind) || model_looks_text_only(upstream_model)
}

pub fn contains_image_blocks(body: &Value) -> bool {
    responses_input_has_image(body.get("input")) || messages_have_image(body.get("messages"))
}

fn responses_input_has_image(input: Option<&Value>) -> bool {
    match input {
        Some(Value::Array(items)) => items.iter().any(item_has_image),
        Some(item @ Value::Object(_)) => item_has_image(item),
        _ => false,
    }
}

fn messages_have_image(messages: Option<&Value>) -> bool {
    let Some(Value::Array(msgs)) = messages else {
        return false;
    };
    msgs.iter().any(item_has_image)
}

fn item_has_image(item: &Value) -> bool {
    if is_image_type(item.get("type").and_then(Value::as_str)) {
        return true;
    }
    match item.get("content") {
        Some(Value::Array(parts)) => parts
            .iter()
            .any(|part| is_image_type(part.get("type").and_then(Value::as_str))),
        _ => false,
    }
}

fn is_image_type(ty: Option<&str>) -> bool {
    matches!(
        ty,
        Some("input_image" | "image" | "image_url" | "output_image")
    )
}

/// Replace image blocks in-place. Returns how many image parts were replaced.
pub fn replace_images_with_marker(body: &mut Value) -> usize {
    let mut count = 0;
    if let Some(input) = body.get_mut("input") {
        count += replace_in_value(input, "input_text");
    }
    if let Some(messages) = body.get_mut("messages") {
        count += replace_in_value(messages, "text");
    }
    count
}

fn replace_in_value(value: &mut Value, text_type: &str) -> usize {
    match value {
        Value::Array(items) => {
            let mut total = 0;
            for item in items.iter_mut() {
                total += replace_in_item(item, text_type);
            }
            total
        }
        Value::Object(_) => replace_in_item(value, text_type),
        _ => 0,
    }
}

fn replace_in_item(item: &mut Value, text_type: &str) -> usize {
    let mut count = 0;
    if is_image_type(item.get("type").and_then(Value::as_str)) {
        replace_image_block(item, text_type);
        return 1;
    }
    if let Some(content) = item.get_mut("content") {
        match content {
            Value::Array(parts) => {
                for part in parts.iter_mut() {
                    if is_image_type(part.get("type").and_then(Value::as_str)) {
                        replace_image_block(part, text_type);
                        count += 1;
                    }
                }
            }
            other if is_image_type(other.get("type").and_then(Value::as_str)) => {
                // rare: content is a single image object
                replace_image_block(other, text_type);
                count += 1;
            }
            _ => {}
        }
    }
    count
}

fn replace_image_block(block: &mut Value, text_type: &str) {
    *block = json!({
        "type": text_type,
        "text": UNSUPPORTED_IMAGE_MARKER,
    });
}

/// True when an upstream error body indicates a text-only modality rejection.
pub fn is_unsupported_image_error_body(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    const SELF_EVIDENT: &[&str] = &["only support text", "only supports text"];
    if SELF_EVIDENT.iter().any(|h| lower.contains(h)) {
        return true;
    }
    let mentions_image = lower.contains("image")
        || lower.contains("vision")
        || lower.contains("multimodal")
        || lower.contains("media");
    if !mentions_image {
        return false;
    }
    lower.contains("not support")
        || lower.contains("unsupported")
        || lower.contains("does not support")
        || lower.contains("cannot process")
        || lower.contains("can't process")
        || lower.contains("invalid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_responses_input_image_for_deepseek() {
        assert!(should_strip_images("deepseek", "deepseek-v4-flash"));
        let mut body = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "look"},
                    {"type": "input_image", "image_url": "data:image/png;base64,abc"}
                ]
            }]
        });
        assert!(contains_image_blocks(&body));
        let n = replace_images_with_marker(&mut body);
        assert_eq!(n, 1);
        assert!(!contains_image_blocks(&body));
        assert_eq!(
            body["input"][0]["content"][1]["text"],
            UNSUPPORTED_IMAGE_MARKER
        );
    }

    #[test]
    fn openai_and_xai_keep_images() {
        assert!(!should_strip_images("openai", "gpt-5.6"));
        assert!(!should_strip_images("xai", "grok-4.5"));
        assert!(!should_strip_images("kimi", "kimi-for-coding"));
    }

    #[test]
    fn detects_text_only_error_phrases() {
        assert!(is_unsupported_image_error_body(
            br#"{"error":{"message":"Model only support text input"}}"#
        ));
        assert!(is_unsupported_image_error_body(
            br#"{"error":{"message":"This model does not support image input"}}"#
        ));
        assert!(!is_unsupported_image_error_body(
            br#"{"error":{"message":"rate limit exceeded"}}"#
        ));
    }
}
