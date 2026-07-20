use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::domain::{OpenAiQuotaSnapshot, QuotaWindow, ResetCreditSummary, ResetCreditsSummary};

const CHATGPT_ORIGIN: &str = "https://chatgpt.com";
/// Headers aligned with Sub2API / Nice Switch Codex quota clients so
/// `/backend-api/wham/usage` accepts the request past Cloudflare checks.
const CODEX_BETA: &str = "codex-1";
const CODEX_ORIGINATOR: &str = "Codex Desktop";
const CODEX_LANGUAGE: &str = "zh-CN";
const USER_AGENT: &str = "Codex-Spur/0.1";

const FIVE_HOUR_SECONDS: i64 = 5 * 60 * 60;
const SEVEN_DAY_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Error)]
pub enum ConsumeResetError {
    #[error("重置卡请求可能已到达上游：{0}")]
    AmbiguousTransport(#[from] reqwest::Error),
    #[error("重置卡接口返回不是 JSON：{0}")]
    AmbiguousResponse(String),
    #[error("{0}")]
    Rejected(String),
}

impl ConsumeResetError {
    pub const fn is_ambiguous(&self) -> bool {
        matches!(
            self,
            Self::AmbiguousTransport(_) | Self::AmbiguousResponse(_)
        )
    }
}

#[derive(Debug, Deserialize)]
struct UsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<AdditionalRateLimit>>,
    /// Some upstream payloads embed reset credits on the usage response.
    #[serde(default)]
    rate_limit_reset_credits: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AdditionalRateLimit {
    rate_limit: Option<RateLimit>,
}

#[derive(Debug, Deserialize, Default)]
struct RateLimit {
    primary_window: Option<RawWindow>,
    secondary_window: Option<RawWindow>,
}

/// One rate-limit window from `/wham/usage`.
/// Fields are optional with defaults so a missing `reset_at` (only
/// `reset_after_seconds`) does not fail the whole snapshot parse.
#[derive(Debug, Clone, Deserialize, Default)]
struct RawWindow {
    #[serde(default)]
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: i64,
    #[serde(default)]
    reset_at: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
}

pub async fn fetch(
    credential_id: &str,
    access_token: &str,
    account_id: &str,
) -> anyhow::Result<OpenAiQuotaSnapshot> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(18))
        .build()?;
    let usage = fetch_first_json(
        &client,
        access_token,
        account_id,
        &[
            // Primary path used by Sub2API and Nice Switch.
            format!("{CHATGPT_ORIGIN}/backend-api/wham/usage"),
            // Legacy Codex CLI surface (kept as fallback only).
            format!("{CHATGPT_ORIGIN}/api/codex/usage"),
        ],
    )
    .await?;
    let payload: UsagePayload = serde_json::from_value(usage).context("额度响应结构无法解析")?;
    let reset_credits = match fetch_first_json(
        &client,
        access_token,
        account_id,
        &[format!(
            "{CHATGPT_ORIGIN}/backend-api/wham/rate-limit-reset-credits"
        )],
    )
    .await
    {
        Ok(value) => Some(map_reset_credits(value)),
        Err(_) => payload.rate_limit_reset_credits.map(map_reset_credits),
    };
    let mut windows = Vec::new();
    collect_windows(payload.rate_limit, &mut windows);
    if let Some(additional) = payload.additional_rate_limits {
        for item in additional {
            collect_windows(item.rate_limit, &mut windows);
        }
    }
    let fetched_at = now();
    let (five_hour, seven_day) = select_quota_windows(&windows, fetched_at);
    Ok(OpenAiQuotaSnapshot {
        credential_id: credential_id.to_string(),
        plan_type: payload.plan_type,
        five_hour,
        seven_day,
        reset_credits,
        fetched_at,
    })
}

pub async fn consume_reset_credit(
    access_token: &str,
    account_id: &str,
    idempotency_key: &str,
) -> Result<Value, ConsumeResetError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(25))
        .build()?;
    let response = client
        .post(format!(
            "{CHATGPT_ORIGIN}/backend-api/wham/rate-limit-reset-credits/consume"
        ))
        .headers(codex_quota_headers(access_token, account_id))
        .header("Idempotency-Key", idempotency_key)
        .json(&serde_json::json!({
            "redeem_request_id": idempotency_key,
        }))
        .send()
        .await?;
    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|error| ConsumeResetError::AmbiguousResponse(error.to_string()))?;
    if !status.is_success() {
        return Err(ConsumeResetError::Rejected(format!(
            "消耗重置卡失败（{}）：{}",
            status,
            safe_message(&payload)
        )));
    }
    Ok(payload)
}

fn codex_quota_headers(access_token: &str, account_id: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, ACCEPT};
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {access_token}")) {
        headers.insert(AUTHORIZATION, value);
    }
    if let Ok(value) = HeaderValue::from_str(account_id) {
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            value.clone(),
        );
        // Title-case alias used by some ChatGPT clients.
        if let Ok(name) = HeaderName::from_bytes(b"ChatGPT-Account-Id") {
            headers.insert(name, value);
        }
    }
    if let Ok(value) = HeaderValue::from_str(CODEX_BETA) {
        headers.insert(HeaderName::from_static("openai-beta"), value);
    }
    if let Ok(value) = HeaderValue::from_str(CODEX_LANGUAGE) {
        headers.insert(HeaderName::from_static("oai-language"), value);
    }
    if let Ok(value) = HeaderValue::from_str(CODEX_ORIGINATOR) {
        headers.insert(HeaderName::from_static("originator"), value);
    }
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("sec-fetch-site"),
        HeaderValue::from_static("none"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-mode"),
        HeaderValue::from_static("no-cors"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-dest"),
        HeaderValue::from_static("empty"),
    );
    if let Ok(value) = HeaderValue::from_str("u=4, i") {
        headers.insert(HeaderName::from_static("priority"), value);
    }
    headers
}

async fn fetch_first_json(
    client: &reqwest::Client,
    access_token: &str,
    account_id: &str,
    urls: &[String],
) -> anyhow::Result<Value> {
    let mut errors = Vec::new();
    for url in urls {
        match client
            .get(url)
            .headers(codex_quota_headers(access_token, account_id))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .context("额度接口响应体读取失败")?;
                return serde_json::from_str(&body).with_context(|| {
                    format!(
                        "额度接口返回不是 JSON（{status}）：{}",
                        body.chars().take(120).collect::<String>()
                    )
                });
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let snippet = body.chars().take(160).collect::<String>();
                errors.push(format!("{url} -> {status} {snippet}"));
            }
            Err(error) => errors.push(format!("{url} -> {error}")),
        }
    }
    Err(anyhow!("额度请求失败：{}", errors.join(" | ")))
}

fn collect_windows(rate_limit: Option<RateLimit>, output: &mut Vec<RawWindow>) {
    if let Some(rate_limit) = rate_limit {
        if let Some(window) = rate_limit.primary_window {
            if window_has_signal(&window) {
                output.push(window);
            }
        }
        if let Some(window) = rate_limit.secondary_window {
            if window_has_signal(&window) {
                output.push(window);
            }
        }
    }
}

fn window_has_signal(window: &RawWindow) -> bool {
    window.limit_window_seconds > 0
        || window.reset_at.is_some_and(|v| v > 0)
        || window.reset_after_seconds.is_some()
        || window.used_percent > 0.0
}

/// Map windows onto the Codex 5h / 7d ladder by nearest `limit_window_seconds`
/// (Sub2API Normalize behaviour). Never force a 7d window into the 5h slot.
fn select_quota_windows(windows: &[RawWindow], fetched_at: i64) -> (Option<QuotaWindow>, Option<QuotaWindow>) {
    let mut five: Option<(i64, RawWindow)> = None;
    let mut seven: Option<(i64, RawWindow)> = None;
    for window in windows {
        let seconds = window.limit_window_seconds;
        if seconds <= 0 {
            // Without a duration, fall back to reset horizon heuristics.
            let reset_hint = window
                .reset_after_seconds
                .or_else(|| {
                    window.reset_at.and_then(|reset| {
                        let reset = normalize_epoch(reset);
                        (reset > fetched_at).then_some(reset - fetched_at)
                    })
                })
                .unwrap_or(0);
            if reset_hint <= 0 {
                continue;
            }
            let d5 = (reset_hint - FIVE_HOUR_SECONDS).abs();
            let d7 = (reset_hint - SEVEN_DAY_SECONDS).abs();
            if d5 <= d7 {
                if five.as_ref().is_none_or(|(dist, _)| d5 < *dist) {
                    five = Some((d5, window.clone()));
                }
            } else if seven.as_ref().is_none_or(|(dist, _)| d7 < *dist) {
                seven = Some((d7, window.clone()));
            }
            continue;
        }
        let d5 = (seconds - FIVE_HOUR_SECONDS).abs();
        let d7 = (seconds - SEVEN_DAY_SECONDS).abs();
        if d5 <= d7 {
            if five.as_ref().is_none_or(|(dist, _)| d5 < *dist) {
                five = Some((d5, window.clone()));
            }
        } else if seven.as_ref().is_none_or(|(dist, _)| d7 < *dist) {
            seven = Some((d7, window.clone()));
        }
    }
    (
        five.map(|(_, window)| to_window(window, fetched_at)),
        seven.map(|(_, window)| to_window(window, fetched_at)),
    )
}

fn to_window(window: RawWindow, fetched_at: i64) -> QuotaWindow {
    let used = normalize_used_percent(window.used_percent);
    let reset_at = effective_reset_at(&window, fetched_at);
    let window_seconds = if window.limit_window_seconds > 0 {
        window.limit_window_seconds
    } else {
        // Best-effort label when upstream only gave a reset countdown.
        window
            .reset_after_seconds
            .filter(|s| *s > 0)
            .unwrap_or(0)
    };
    QuotaWindow {
        used_percent: used,
        remaining_percent: (100.0 - used).clamp(0.0, 100.0),
        reset_at,
        window_seconds,
    }
}

/// ChatGPT usually reports 0–100. Some edge payloads use a 0–1 fraction.
fn normalize_used_percent(raw: f64) -> f64 {
    if (0.0..1.0).contains(&raw) && raw != 0.0 {
        // Ambiguous: 0.42 could mean 0.42% or 42%. Real Plus/Pro usage
        // payloads use full percent (e.g. 17.5). Fractions appear in some
        // spark/feature meters; treat values strictly in (0,1) as percent
        // only when they look like percentage points would be unusable —
        // keep as-is when already tiny, multiply only if clearly a ratio
        // is expected. Sub2API stores body values as-is for wham/usage.
        // Prefer treating as 0–100: 0.42 stays 0.42%.
        raw.clamp(0.0, 100.0)
    } else {
        raw.clamp(0.0, 100.0)
    }
}

fn effective_reset_at(window: &RawWindow, fetched_at: i64) -> Option<i64> {
    if let Some(reset_at) = window.reset_at {
        if reset_at > 0 {
            return Some(normalize_epoch(reset_at));
        }
    }
    window
        .reset_after_seconds
        .map(|seconds| fetched_at + seconds.max(0))
}

fn map_reset_credits(payload: Value) -> ResetCreditsSummary {
    let available_count = payload
        .get("available_count")
        .or_else(|| payload.get("availableCount"))
        .and_then(parse_i64);
    let credits = payload
        .get("credits")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| ResetCreditSummary {
                    granted_at: timestamp(
                        item,
                        &["granted_at", "grantedAt", "created_at", "createdAt"],
                    ),
                    expires_at: timestamp(
                        item,
                        &["expires_at", "expiresAt", "expiration", "expire_at"],
                    ),
                })
                .collect()
        })
        .unwrap_or_default();
    ResetCreditsSummary {
        available_count,
        credits,
    }
}

fn timestamp(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(parse_i64))
        .map(normalize_epoch)
}

fn parse_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn normalize_epoch(value: i64) -> i64 {
    if value > 10_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn safe_message(payload: &Value) -> String {
    payload
        .pointer("/error/message")
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("上游拒绝了操作")
        .chars()
        .take(200)
        .collect()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_primary_secondary_by_window_duration() {
        let windows = vec![
            RawWindow {
                used_percent: 20.0,
                limit_window_seconds: 18_000,
                reset_at: Some(1_800_000_000),
                reset_after_seconds: None,
            },
            RawWindow {
                used_percent: 40.0,
                limit_window_seconds: 604_800,
                reset_at: Some(1_800_000_100),
                reset_after_seconds: None,
            },
        ];
        let (five, seven) = select_quota_windows(&windows, 1_700_000_000);
        assert_eq!(five.as_ref().unwrap().used_percent, 20.0);
        assert_eq!(five.as_ref().unwrap().remaining_percent, 80.0);
        assert_eq!(five.as_ref().unwrap().window_seconds, 18_000);
        assert_eq!(seven.as_ref().unwrap().used_percent, 40.0);
        assert_eq!(seven.as_ref().unwrap().window_seconds, 604_800);
    }

    #[test]
    fn does_not_put_weekly_window_into_five_hour_slot() {
        let windows = vec![RawWindow {
            used_percent: 55.0,
            limit_window_seconds: 604_800,
            reset_at: Some(1_800_000_000),
            reset_after_seconds: None,
        }];
        let (five, seven) = select_quota_windows(&windows, 1_700_000_000);
        assert!(five.is_none());
        assert_eq!(seven.as_ref().unwrap().used_percent, 55.0);
    }

    #[test]
    fn accepts_swapped_primary_secondary_order() {
        // ChatGPT often puts 7d in primary and 5h in secondary.
        let windows = vec![
            RawWindow {
                used_percent: 12.0,
                limit_window_seconds: 604_800,
                reset_at: None,
                reset_after_seconds: Some(86_400),
            },
            RawWindow {
                used_percent: 3.0,
                limit_window_seconds: 18_000,
                reset_at: None,
                reset_after_seconds: Some(3_600),
            },
        ];
        let fetched = 1_700_000_000;
        let (five, seven) = select_quota_windows(&windows, fetched);
        assert_eq!(five.as_ref().unwrap().used_percent, 3.0);
        assert_eq!(five.as_ref().unwrap().reset_at, Some(fetched + 3_600));
        assert_eq!(seven.as_ref().unwrap().used_percent, 12.0);
        assert_eq!(seven.as_ref().unwrap().reset_at, Some(fetched + 86_400));
    }

    #[test]
    fn parses_usage_payload_without_reset_at() {
        let json = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 17.5,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 7200
                },
                "secondary_window": {
                    "used_percent": 42,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 500000
                }
            }
        }"#;
        let payload: UsagePayload = serde_json::from_str(json).expect("parse");
        let mut windows = Vec::new();
        collect_windows(payload.rate_limit, &mut windows);
        let (five, seven) = select_quota_windows(&windows, 1_000);
        assert_eq!(five.unwrap().used_percent, 17.5);
        assert_eq!(seven.unwrap().used_percent, 42.0);
    }

    #[test]
    fn null_windows_do_not_fail_parse() {
        let json = r#"{
            "plan_type": "free",
            "rate_limit": {
                "primary_window": null,
                "secondary_window": null
            }
        }"#;
        let payload: UsagePayload = serde_json::from_str(json).expect("parse");
        let mut windows = Vec::new();
        collect_windows(payload.rate_limit, &mut windows);
        assert!(windows.is_empty());
        let (five, seven) = select_quota_windows(&windows, now());
        assert!(five.is_none());
        assert!(seven.is_none());
    }
}
