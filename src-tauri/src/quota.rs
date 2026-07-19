use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::domain::{OpenAiQuotaSnapshot, QuotaWindow, ResetCreditSummary, ResetCreditsSummary};

const CHATGPT_ORIGIN: &str = "https://chatgpt.com";

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
    additional_rate_limits: Option<Vec<AdditionalRateLimit>>,
}

#[derive(Debug, Deserialize)]
struct AdditionalRateLimit {
    rate_limit: Option<RateLimit>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    primary_window: Option<RawWindow>,
    secondary_window: Option<RawWindow>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawWindow {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: i64,
}

pub async fn fetch(
    credential_id: &str,
    access_token: &str,
    account_id: &str,
) -> anyhow::Result<OpenAiQuotaSnapshot> {
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .timeout(std::time::Duration::from_secs(18))
        .build()?;
    let usage = fetch_first_json(
        &client,
        access_token,
        account_id,
        &[
            format!("{CHATGPT_ORIGIN}/backend-api/wham/usage"),
            format!("{CHATGPT_ORIGIN}/api/codex/usage"),
        ],
    )
    .await?;
    let payload: UsagePayload = serde_json::from_value(usage).context("额度响应结构无法解析")?;
    let reset_credits = fetch_first_json(
        &client,
        access_token,
        account_id,
        &[format!(
            "{CHATGPT_ORIGIN}/backend-api/wham/rate-limit-reset-credits"
        )],
    )
    .await
    .ok()
    .map(map_reset_credits);
    let mut windows = Vec::new();
    collect_windows(payload.rate_limit, &mut windows);
    if let Some(additional) = payload.additional_rate_limits {
        for item in additional {
            collect_windows(item.rate_limit, &mut windows);
        }
    }
    Ok(OpenAiQuotaSnapshot {
        credential_id: credential_id.to_string(),
        plan_type: payload.plan_type,
        five_hour: nearest_window(&windows, 5 * 60 * 60).map(to_window),
        seven_day: nearest_window(&windows, 7 * 24 * 60 * 60).map(to_window),
        reset_credits,
        fetched_at: now(),
    })
}

pub async fn consume_reset_credit(
    access_token: &str,
    account_id: &str,
    idempotency_key: &str,
) -> Result<Value, ConsumeResetError> {
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .timeout(std::time::Duration::from_secs(25))
        .build()?;
    let response = client
        .post(format!(
            "{CHATGPT_ORIGIN}/backend-api/wham/rate-limit-reset-credits/consume"
        ))
        .bearer_auth(access_token)
        .header("ChatGPT-Account-Id", account_id)
        .header("Idempotency-Key", idempotency_key)
        .header("Accept", "application/json")
        .json(&serde_json::json!({}))
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
            .bearer_auth(access_token)
            .header("ChatGPT-Account-Id", account_id)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                return response.json().await.context("额度接口返回不是 JSON")
            }
            Ok(response) => errors.push(format!("{} -> {}", url, response.status())),
            Err(error) => errors.push(format!("{} -> {}", url, error)),
        }
    }
    Err(anyhow!("额度请求失败：{}", errors.join(" | ")))
}

fn collect_windows(rate_limit: Option<RateLimit>, output: &mut Vec<RawWindow>) {
    if let Some(rate_limit) = rate_limit {
        if let Some(window) = rate_limit.primary_window {
            output.push(window);
        }
        if let Some(window) = rate_limit.secondary_window {
            output.push(window);
        }
    }
}

fn nearest_window(windows: &[RawWindow], target: i64) -> Option<RawWindow> {
    windows
        .iter()
        .min_by_key(|window| (window.limit_window_seconds - target).abs())
        .cloned()
}

fn to_window(window: RawWindow) -> QuotaWindow {
    QuotaWindow {
        used_percent: window.used_percent,
        remaining_percent: (100.0 - window.used_percent).clamp(0.0, 100.0),
        reset_at: Some(normalize_epoch(window.reset_at)),
        window_seconds: window.limit_window_seconds,
    }
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
    fn maps_nearest_quota_windows() {
        let windows = vec![
            RawWindow {
                used_percent: 20.0,
                limit_window_seconds: 18_000,
                reset_at: 1_800_000_000,
            },
            RawWindow {
                used_percent: 40.0,
                limit_window_seconds: 604_800,
                reset_at: 1_800_000_100,
            },
        ];
        assert_eq!(nearest_window(&windows, 18_000).unwrap().used_percent, 20.0);
        assert_eq!(to_window(windows[0].clone()).remaining_percent, 80.0);
    }
}
