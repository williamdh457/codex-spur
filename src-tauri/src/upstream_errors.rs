//! Parse OpenAI / Codex upstream rate-limit and usage-limit signals.
//! Independent of Sub2API; reproduces observable body/header contracts only.

use reqwest::header::HeaderMap;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Parsed cooldown decision for a rate / usage limit response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitCooldown {
    /// Absolute unix seconds when the account may be scheduled again.
    pub cooldown_until: i64,
    /// Human-readable reason (no secrets).
    pub reason: &'static str,
    /// True when the body looked like usage_limit_reached / rate_limit_exceeded.
    pub is_usage_limit: bool,
}

/// Whether this HTTP status should trigger account failover.
#[allow(dead_code)]
pub fn is_failover_status(status: reqwest::StatusCode) -> bool {
    is_failover_status_with_options(status, false)
}

/// Failover check with Sub2API-like `failover_on_400` option.
pub fn is_failover_status_with_options(
    status: reqwest::StatusCode,
    failover_on_400: bool,
) -> bool {
    let code = status.as_u16();
    if matches!(code, 401 | 402 | 403 | 429 | 529) || status.is_server_error() {
        return true;
    }
    // Narrow 400 failover: only when explicitly enabled (Sub2API default off).
    failover_on_400 && code == 400
}

pub fn status_category(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 => "auth_invalid",
        402 => "payment_required",
        403 => "entitlement",
        429 => "rate_limited",
        529 => "overloaded",
        code if (500..600).contains(&code) => "upstream_5xx",
        code if (400..500).contains(&code) => "upstream_4xx",
        _ => "ok",
    }
}

/// Compute cooldown from Retry-After, x-codex-* headers, and JSON body.
pub fn resolve_rate_limit_cooldown(
    headers: &HeaderMap,
    body: Option<&[u8]>,
    default_secs: i64,
    now_unix: i64,
) -> RateLimitCooldown {
    let default_secs = default_secs.max(1);

    if let Some(until) = parse_codex_reset_headers(headers, now_unix) {
        return RateLimitCooldown {
            cooldown_until: until.max(now_unix + 1),
            reason: "x-codex rate-limit headers",
            is_usage_limit: true,
        };
    }

    if let Some(secs) = parse_retry_after_secs(headers) {
        return RateLimitCooldown {
            cooldown_until: now_unix + secs.max(1),
            reason: "Retry-After header",
            is_usage_limit: false,
        };
    }

    if let Some(body) = body {
        if let Some((until, usage)) = parse_openai_usage_limit_reset(body, now_unix) {
            return RateLimitCooldown {
                cooldown_until: until.max(now_unix + 1),
                reason: if usage {
                    "usage_limit_reached body"
                } else {
                    "rate_limit body"
                },
                is_usage_limit: usage,
            };
        }
    }

    RateLimitCooldown {
        cooldown_until: now_unix + default_secs,
        reason: "default 429 cooldown",
        is_usage_limit: false,
    }
}

/// Body looks like an OpenAI usage / rate limit error (regardless of HTTP status).
pub fn body_is_usage_or_rate_limit(body: &[u8]) -> bool {
    parse_openai_usage_limit_reset(body, now_unix()).is_some()
        || error_type_is_usage_or_rate_limit(body)
}

fn error_type_is_usage_or_rate_limit(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let err = value.get("error").unwrap_or(&value);
    let ty = err
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let code = err
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let msg = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    matches_usage_or_rate_limit(&ty, &code, &msg)
}

fn matches_usage_or_rate_limit(ty: &str, code: &str, msg: &str) -> bool {
    if ty.contains("usage_limit")
        || ty.contains("rate_limit")
        || code.contains("usage_limit")
        || code.contains("rate_limit")
        || code.contains("insufficient_quota")
    {
        return true;
    }
    (msg.contains("usage limit") && msg.contains("reached"))
        || (msg.contains("rate limit") && (msg.contains("reached") || msg.contains("exceeded")))
}

/// Returns (cooldown_until, is_usage_limit).
fn parse_openai_usage_limit_reset(body: &[u8], now_unix: i64) -> Option<(i64, bool)> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let err = value.get("error").unwrap_or(&value);
    let ty = err
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let code = err
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let msg = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches_usage_or_rate_limit(&ty, &code, &msg) {
        return None;
    }
    let is_usage = ty.contains("usage_limit")
        || code.contains("usage_limit")
        || (msg.contains("usage limit") && msg.contains("reached"));

    if let Some(ts) = json_i64(err.get("resets_at")) {
        return Some((ts, is_usage));
    }
    if let Some(secs) = json_i64(err.get("resets_in_seconds")) {
        return Some((now_unix + secs.max(1), is_usage));
    }
    // Recognized as usage/rate limit but no reset → caller applies default.
    Some((now_unix + 30, is_usage))
}

fn parse_retry_after_secs(headers: &HeaderMap) -> Option<i64> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())?
        .trim();
    if let Ok(secs) = raw.parse::<i64>() {
        return Some(secs.max(1));
    }
    // HTTP-date form is rare for OpenAI; skip for v1.
    None
}

/// Prefer exhausted window (used >= 100) reset-after; else soonest positive reset-after.
fn parse_codex_reset_headers(headers: &HeaderMap, now_unix: i64) -> Option<i64> {
    let windows = [
        ("x-codex-primary-used-percent", "x-codex-primary-reset-after-seconds"),
        ("x-codex-secondary-used-percent", "x-codex-secondary-reset-after-seconds"),
        ("x-codex-7d-used-percent", "x-codex-7d-reset-after-seconds"),
        ("x-codex-5h-used-percent", "x-codex-5h-reset-after-seconds"),
    ];
    let mut exhausted: Option<i64> = None;
    let mut any: Option<i64> = None;
    for (used_key, reset_key) in windows {
        let used = header_f64(headers, used_key);
        let reset_after = header_i64(headers, reset_key);
        let Some(reset_after) = reset_after.filter(|s| *s > 0) else {
            continue;
        };
        let until = now_unix + reset_after;
        if used.is_some_and(|u| u >= 100.0) {
            exhausted = Some(match exhausted {
                Some(prev) => prev.max(until),
                None => until,
            });
        }
        any = Some(match any {
            Some(prev) => prev.min(until),
            None => until,
        });
    }
    exhausted.or(any)
}

fn header_f64(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok())
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_u64() {
        return Some(n as i64);
    }
    if let Some(n) = value.as_f64() {
        return Some(n as i64);
    }
    if let Some(s) = value.as_str() {
        return s.trim().parse().ok();
    }
    None
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Stable content-derived session seed (Sub2API-like observable contract).
/// Uses model + tools summary + instructions + first user text so multi-turn
/// chats that keep the first user message share the same seed.
pub fn content_session_seed(body: &Value) -> Option<String> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let tools = body
        .get("tools")
        .map(|t| truncate_stable(&t.to_string(), 512))
        .unwrap_or_default();
    let first_user = first_user_text(body);
    if model.is_empty() && instructions.is_empty() && tools.is_empty() && first_user.is_empty() {
        return None;
    }
    Some(format!(
        "model={model}|tools={tools}|system={}|first_user={first_user}",
        truncate_stable(instructions, 256),
    ))
}

fn first_user_text(body: &Value) -> String {
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for msg in messages {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "user" {
                return truncate_stable(&content_as_text(msg.get("content")), 256);
            }
        }
    }
    if let Some(input) = body.get("input") {
        if let Some(arr) = input.as_array() {
            for item in arr {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("");
                let ty = item.get("type").and_then(Value::as_str).unwrap_or("");
                if role == "user" || ty == "message" {
                    let text = content_as_text(item.get("content"));
                    if !text.is_empty() {
                        return truncate_stable(&text, 256);
                    }
                }
            }
        } else if let Some(s) = input.as_str() {
            return truncate_stable(s, 256);
        }
    }
    String::new()
}

fn content_as_text(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut out = String::new();
        for part in arr {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(t);
            } else if let Some(t) = part.as_str() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(t);
            }
        }
        return out;
    }
    String::new()
}

fn truncate_stable(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use serde_json::json;

    #[test]
    fn parses_usage_limit_resets_at() {
        let body = br#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","resets_at":1800000000}}"#;
        let decision = resolve_rate_limit_cooldown(&HeaderMap::new(), Some(body), 30, 1_700_000_000);
        assert_eq!(decision.cooldown_until, 1_800_000_000);
        assert!(decision.is_usage_limit);
    }

    #[test]
    fn parses_resets_in_seconds() {
        let body = br#"{"error":{"type":"rate_limit_exceeded","resets_in_seconds":3600}}"#;
        let now = 1_700_000_000;
        let decision = resolve_rate_limit_cooldown(&HeaderMap::new(), Some(body), 30, now);
        assert_eq!(decision.cooldown_until, now + 3600);
    }

    #[test]
    fn prefers_codex_header_over_default() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "x-codex-primary-reset-after-seconds",
            HeaderValue::from_static("7200"),
        );
        let now = 1_700_000_000;
        let decision = resolve_rate_limit_cooldown(&headers, None, 30, now);
        assert_eq!(decision.cooldown_until, now + 7200);
    }

    #[test]
    fn retry_after_used_when_no_body() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("45"));
        let now = 1_700_000_000;
        let decision = resolve_rate_limit_cooldown(&headers, None, 30, now);
        assert_eq!(decision.cooldown_until, now + 45);
    }

    #[test]
    fn failover_includes_5xx_and_402() {
        assert!(is_failover_status(reqwest::StatusCode::PAYMENT_REQUIRED));
        assert!(is_failover_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_failover_status(reqwest::StatusCode::from_u16(529).unwrap()));
        assert!(!is_failover_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!is_failover_status_with_options(
            reqwest::StatusCode::BAD_REQUEST,
            false
        ));
        assert!(is_failover_status_with_options(
            reqwest::StatusCode::BAD_REQUEST,
            true
        ));
    }

    #[test]
    fn content_seed_stable_across_later_turns() {
        let turn1 = json!({
            "model": "gpt-5.4",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ]
        });
        let turn2 = json!({
            "model": "gpt-5.4",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi"},
                {"role": "user", "content": "How are you?"}
            ]
        });
        assert_eq!(content_session_seed(&turn1), content_session_seed(&turn2));
    }

    #[test]
    fn content_seed_differs_on_first_user() {
        let a = json!({"model":"gpt-5","messages":[{"role":"user","content":"A"}]});
        let b = json!({"model":"gpt-5","messages":[{"role":"user","content":"B"}]});
        assert_ne!(content_session_seed(&a), content_session_seed(&b));
    }
}
