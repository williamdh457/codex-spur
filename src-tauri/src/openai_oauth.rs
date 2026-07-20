//! OpenAI ChatGPT / Codex subscription device-code login.
//!
//! Independent implementation of the public Codex CLI device-auth flow.
//! Tokens are returned to the caller for vault storage; nothing is logged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// Public Codex CLI OAuth client id.
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_AUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const USER_AGENT: &str = "codex_cli_rs/0.144.1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoginPoll {
    pub status: String,
    pub tokens: Option<DeviceLoginTokens>,
    pub message: Option<String>,
}

#[derive(Clone)]
struct PendingDevice {
    user_code: String,
    expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct OpenAiOAuthManager {
    pending: Arc<RwLock<HashMap<String, PendingDevice>>>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    interval: Option<Value>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DevicePollSuccess {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl OpenAiOAuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_device_login(&self) -> Result<DeviceLoginStart, String> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .post(DEVICE_AUTH_USERCODE_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "client_id": CODEX_CLIENT_ID }))
            .send()
            .await
            .map_err(|e| format!("启动 OpenAI 登录失败：{e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let snippet = text.chars().take(200).collect::<String>();
            return Err(format!("启动 OpenAI 登录失败（{status}）：{snippet}"));
        }
        let device: DeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 Device Code 失败：{e}"))?;
        let expires_in = device.expires_in.unwrap_or(900);
        let interval_secs = parse_interval(device.interval.as_ref()).max(3);
        {
            let mut pending = self.pending.write().await;
            let now = Instant::now();
            pending.retain(|_, entry| entry.expires_at > now);
            pending.insert(
                device.device_auth_id.clone(),
                PendingDevice {
                    user_code: device.user_code.clone(),
                    expires_at: now + Duration::from_secs(expires_in),
                },
            );
        }
        Ok(DeviceLoginStart {
            device_code: device.device_auth_id,
            user_code: device.user_code,
            verification_uri: DEVICE_VERIFICATION_URL.to_string(),
            interval_secs,
            expires_in,
        })
    }

    pub async fn poll_device_login(&self, device_code: &str) -> Result<DeviceLoginPoll, String> {
        let entry = {
            let pending = self.pending.read().await;
            pending.get(device_code).cloned()
        };
        let Some(entry) = entry else {
            return Ok(DeviceLoginPoll {
                status: "error".into(),
                tokens: None,
                message: Some("登录会话不存在，请重新开始".into()),
            });
        };
        if entry.expires_at <= Instant::now() {
            let mut pending = self.pending.write().await;
            pending.remove(device_code);
            return Ok(DeviceLoginPoll {
                status: "expired".into(),
                tokens: None,
                message: Some("登录已过期，请重新开始".into()),
            });
        }

        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .post(DEVICE_AUTH_TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_auth_id": device_code,
                "user_code": entry.user_code,
            }))
            .send()
            .await
            .map_err(|e| format!("轮询登录状态失败：{e}"))?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(DeviceLoginPoll {
                status: "pending".into(),
                tokens: None,
                message: None,
            });
        }
        if status == reqwest::StatusCode::GONE {
            let mut pending = self.pending.write().await;
            pending.remove(device_code);
            return Ok(DeviceLoginPoll {
                status: "expired".into(),
                tokens: None,
                message: Some("登录已过期，请重新开始".into()),
            });
        }
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let snippet = text.chars().take(200).collect::<String>();
            return Ok(DeviceLoginPoll {
                status: "error".into(),
                tokens: None,
                message: Some(format!("登录轮询失败（{status}）：{snippet}")),
            });
        }

        let success: DevicePollSuccess = response
            .json()
            .await
            .map_err(|e| format!("解析授权响应失败：{e}"))?;
        let tokens = exchange_code_for_tokens(&client, &success.authorization_code, &success.code_verifier)
            .await?;
        {
            let mut pending = self.pending.write().await;
            pending.remove(device_code);
        }
        let (account_id, email) = extract_identity(&tokens);
        let account_id = account_id.ok_or_else(|| "无法从 token 解析 ChatGPT account_id".to_string())?;
        let expires_at = tokens
            .expires_in
            .map(|secs| chrono_now_secs().saturating_add(secs as i64));
        Ok(DeviceLoginPoll {
            status: "success".into(),
            tokens: Some(DeviceLoginTokens {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                id_token: tokens.id_token,
                account_id,
                email,
                expires_at,
            }),
            message: None,
        })
    }

    pub async fn cancel_device_login(&self, device_code: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(device_code);
    }
}

async fn exchange_code_for_tokens(
    client: &reqwest::Client,
    code: &str,
    code_verifier: &str,
) -> Result<OAuthTokenResponse, String> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding_encode(code),
        urlencoding_encode(DEVICE_REDIRECT_URI),
        urlencoding_encode(CODEX_CLIENT_ID),
        urlencoding_encode(code_verifier),
    );
    let response = client
        .post(OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("换取 token 失败：{e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let snippet = text.chars().take(200).collect::<String>();
        return Err(format!("换取 token 失败（{status}）：{snippet}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("解析 token 响应失败：{e}"))
}

fn parse_interval(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(5),
        Some(Value::String(s)) => s.parse().unwrap_or(5),
        _ => 5,
    }
}

fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn urlencoding_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn extract_identity(tokens: &OAuthTokenResponse) -> (Option<String>, Option<String>) {
    let mut account_id = None;
    let mut email = None;
    for token in [tokens.id_token.as_deref(), Some(tokens.access_token.as_str())]
        .into_iter()
        .flatten()
    {
        if let Some(claims) = parse_jwt_payload(token) {
            if account_id.is_none() {
                account_id = claims
                    .get("https://api.openai.com/auth")
                    .and_then(|a| a.get("chatgpt_account_id"))
                    .and_then(Value::as_str)
                    .or_else(|| claims.get("chatgpt_account_id").and_then(Value::as_str))
                    .map(str::to_string);
            }
            if email.is_none() {
                email = claims
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        claims
                            .get("https://api.openai.com/profile")
                            .and_then(|p| p.get("email"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
            }
        }
    }
    (account_id, email)
}

fn parse_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    serde_json::from_slice(&decoded).ok()
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    // Minimal base64 decoder for JWT payload only.
    const TABLE: &[u8; 128] = &{
        let mut t = [0xffu8; 128];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            t[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        i = 0;
        while i < 10 {
            t[(b'0' + i) as usize] = 52 + i;
            i += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf = [0u8; 4];
    let mut n = 0;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        if b as usize >= TABLE.len() || TABLE[b as usize] == 0xff {
            return None;
        }
        buf[n] = TABLE[b as usize];
        n += 1;
        if n == 4 {
            out.push((buf[0] << 2) | (buf[1] >> 4));
            out.push((buf[1] << 4) | (buf[2] >> 2));
            out.push((buf[2] << 6) | buf[3]);
            n = 0;
        }
    }
    if n == 2 {
        out.push((buf[0] << 2) | (buf[1] >> 4));
    } else if n == 3 {
        out.push((buf[0] << 2) | (buf[1] >> 4));
        out.push((buf[1] << 4) | (buf[2] >> 2));
    } else if n == 1 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_jwt_payload() {
        // header.payload.sig with payload {"chatgpt_account_id":"acc-1","email":"a@b.c"}
        let payload = "eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2MtMSIsImVtYWlsIjoiYUBiLmMifQ";
        let token = format!("xxx.{payload}.yyy");
        let claims = parse_jwt_payload(&token).expect("claims");
        assert_eq!(
            claims.get("chatgpt_account_id").and_then(Value::as_str),
            Some("acc-1")
        );
        assert_eq!(claims.get("email").and_then(Value::as_str), Some("a@b.c"));
    }
}
