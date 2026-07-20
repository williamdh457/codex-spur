//! xAI / Grok subscription device-code OAuth (RFC 8628).
//!
//! Independent implementation of the public Grok-CLI OAuth client that OpenCode
//! also reuses. Tokens are returned for vault storage; secrets are never logged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// Public Grok-CLI OAuth client id (allowlisted by xAI for desktop OAuth).
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.x.ai/oauth2/device/code";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const USER_AGENT: &str = "Codex-Spur/0.1";
const DEFAULT_INTERVAL_SECS: u64 = 5;
const DEFAULT_EXPIRES_SECS: u64 = 300;

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
    /// Optional updated poll interval after `slow_down` (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
}

#[derive(Clone)]
struct PendingDevice {
    expires_at: Instant,
    interval_secs: u64,
}

#[derive(Clone, Default)]
pub struct XaiOAuthManager {
    pending: Arc<RwLock<HashMap<String, PendingDevice>>>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<Value>,
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
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

impl XaiOAuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_device_login(&self) -> Result<DeviceLoginStart, String> {
        let client = http_client()?;
        let body = format!(
            "client_id={}&scope={}",
            urlencoding_encode(CLIENT_ID),
            urlencoding_encode(SCOPE),
        );
        let response = client
            .post(DEVICE_AUTHORIZATION_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("启动 Grok 登录失败：{e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let snippet = text.chars().take(200).collect::<String>();
            return Err(format!("启动 Grok 登录失败（{status}）：{snippet}"));
        }
        let device: DeviceCodeResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 Device Code 失败：{e}"))?;
        if device.device_code.is_empty() || device.user_code.is_empty() {
            return Err("xAI Device Code 响应缺少 device_code / user_code".into());
        }
        let expires_in = device
            .expires_in
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_EXPIRES_SECS);
        let interval_secs = parse_interval(device.interval.as_ref()).max(1);
        {
            let mut pending = self.pending.write().await;
            let now = Instant::now();
            pending.retain(|_, entry| entry.expires_at > now);
            pending.insert(
                device.device_code.clone(),
                PendingDevice {
                    expires_at: now + Duration::from_secs(expires_in),
                    interval_secs,
                },
            );
        }
        let verification_uri = device
            .verification_uri_complete
            .filter(|u| !u.trim().is_empty())
            .unwrap_or(device.verification_uri);
        Ok(DeviceLoginStart {
            device_code: device.device_code,
            user_code: device.user_code,
            verification_uri,
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
                interval_secs: None,
            });
        };
        if entry.expires_at <= Instant::now() {
            let mut pending = self.pending.write().await;
            pending.remove(device_code);
            return Ok(DeviceLoginPoll {
                status: "expired".into(),
                tokens: None,
                message: Some("登录已过期，请重新开始".into()),
                interval_secs: None,
            });
        }

        let client = http_client()?;
        let body = format!(
            "grant_type={}&client_id={}&device_code={}",
            urlencoding_encode(DEVICE_CODE_GRANT),
            urlencoding_encode(CLIENT_ID),
            urlencoding_encode(device_code),
        );
        let response = client
            .post(TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("轮询 Grok 登录状态失败：{e}"))?;

        let status = response.status();
        let payload: OAuthTokenResponse = response
            .json()
            .await
            .map_err(|e| format!("解析 Grok 授权响应失败：{e}"))?;

        if status.is_success() && payload.error.is_none() && !payload.access_token.is_empty() {
            {
                let mut pending = self.pending.write().await;
                pending.remove(device_code);
            }
            let (account_id, email) = extract_identity(&payload);
            let account_id = account_id.unwrap_or_else(|| {
                // Stable opaque fallback when JWT lacks principal_id.
                format!("xai-{}", short_fingerprint(&payload.access_token))
            });
            let expires_at = payload
                .expires_in
                .map(|secs| chrono_now_secs().saturating_add(secs as i64))
                .or_else(|| jwt_exp_secs(&payload.access_token));
            return Ok(DeviceLoginPoll {
                status: "success".into(),
                tokens: Some(DeviceLoginTokens {
                    access_token: payload.access_token,
                    refresh_token: payload.refresh_token,
                    id_token: payload.id_token,
                    account_id,
                    email,
                    expires_at,
                }),
                message: None,
                interval_secs: None,
            });
        }

        let error = payload.error.as_deref().unwrap_or("");
        match error {
            "authorization_pending" => Ok(DeviceLoginPoll {
                status: "pending".into(),
                tokens: None,
                message: None,
                interval_secs: Some(entry.interval_secs),
            }),
            "slow_down" => {
                let next = entry.interval_secs.saturating_add(5).max(DEFAULT_INTERVAL_SECS);
                {
                    let mut pending = self.pending.write().await;
                    if let Some(slot) = pending.get_mut(device_code) {
                        slot.interval_secs = next;
                    }
                }
                Ok(DeviceLoginPoll {
                    status: "pending".into(),
                    tokens: None,
                    message: Some("授权服务器要求放慢轮询…".into()),
                    interval_secs: Some(next),
                })
            }
            "access_denied" | "authorization_denied" => {
                let mut pending = self.pending.write().await;
                pending.remove(device_code);
                Ok(DeviceLoginPoll {
                    status: "error".into(),
                    tokens: None,
                    message: Some("用户拒绝了 Grok 授权".into()),
                    interval_secs: None,
                })
            }
            "expired_token" => {
                let mut pending = self.pending.write().await;
                pending.remove(device_code);
                Ok(DeviceLoginPoll {
                    status: "expired".into(),
                    tokens: None,
                    message: Some("Device Code 已过期，请重新开始".into()),
                    interval_secs: None,
                })
            }
            other => {
                let detail = payload
                    .error_description
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if other.is_empty() {
                            format!("HTTP {status}")
                        } else {
                            other.to_string()
                        }
                    });
                // Don't leak full error bodies that might contain tokens.
                let snippet = detail.chars().take(200).collect::<String>();
                Ok(DeviceLoginPoll {
                    status: "error".into(),
                    tokens: None,
                    message: Some(format!("Grok 登录失败：{snippet}")),
                    interval_secs: None,
                })
            }
        }
    }

    pub async fn cancel_device_login(&self, device_code: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(device_code);
    }
}

/// Refresh a Grok / xAI OAuth access token via the public CLI client.
///
/// Uses the same `client_id` and `scope` as device login. When the server omits
/// a rotated `refresh_token`, the previous refresh token is preserved.
pub async fn refresh_xai_tokens(refresh_token: &str) -> Result<DeviceLoginTokens, String> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err("缺少 refresh_token".into());
    }
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}&scope={}",
        urlencoding_encode(refresh_token),
        urlencoding_encode(CLIENT_ID),
        urlencoding_encode(SCOPE),
    );
    let client = http_client()?;
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("刷新 Grok token 失败：{e}"))?;

    let status = response.status();
    let payload: OAuthTokenResponse = response
        .json()
        .await
        .map_err(|e| format!("解析 Grok 刷新响应失败：{e}"))?;

    if !status.is_success() || payload.error.is_some() || payload.access_token.is_empty() {
        let detail = payload
            .error_description
            .filter(|s| !s.is_empty())
            .or(payload.error)
            .unwrap_or_else(|| format!("HTTP {status}"));
        let snippet = detail.chars().take(200).collect::<String>();
        return Err(format!("刷新 Grok token 失败（{status}）：{snippet}"));
    }

    let (account_id, email) = extract_identity(&payload);
    let account_id = account_id.unwrap_or_else(|| {
        format!("xai-{}", short_fingerprint(&payload.access_token))
    });
    let expires_at = payload
        .expires_in
        .map(|secs| chrono_now_secs().saturating_add(secs as i64))
        .or_else(|| jwt_exp_secs(&payload.access_token));
    let mut new_refresh = payload.refresh_token.filter(|s| !s.trim().is_empty());
    if new_refresh.is_none() {
        new_refresh = Some(refresh_token.to_string());
    }
    Ok(DeviceLoginTokens {
        access_token: payload.access_token,
        refresh_token: new_refresh,
        id_token: payload.id_token,
        account_id,
        email,
        expires_at,
    })
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

fn parse_interval(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f as u64))
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS),
        Some(Value::String(s)) => s
            .parse()
            .ok()
            .filter(|v: &u64| *v > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS),
        _ => DEFAULT_INTERVAL_SECS,
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
                    .get("principal_id")
                    .and_then(Value::as_str)
                    .or_else(|| claims.get("sub").and_then(Value::as_str))
                    .or_else(|| claims.get("team_id").and_then(Value::as_str))
                    .map(str::to_string);
            }
            if email.is_none() {
                email = claims
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
    }
    (account_id, email)
}

fn jwt_exp_secs(token: &str) -> Option<i64> {
    let claims = parse_jwt_payload(token)?;
    claims.get("exp").and_then(Value::as_i64)
}

fn parse_jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    serde_json::from_slice(&decoded).ok()
}

fn short_fingerprint(token: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    token.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
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
    fn decodes_xai_style_jwt_claims() {
        // {"principal_id":"p-1","email":"a@b.c","team_id":"t-1"}
        let payload = "eyJwcmluY2lwYWxfaWQiOiJwLTEiLCJlbWFpbCI6ImFAYi5jIiwidGVhbV9pZCI6InQtMSJ9";
        let token = format!("xxx.{payload}.yyy");
        let claims = parse_jwt_payload(&token).expect("claims");
        assert_eq!(
            claims.get("principal_id").and_then(Value::as_str),
            Some("p-1")
        );
        assert_eq!(claims.get("email").and_then(Value::as_str), Some("a@b.c"));
    }

    #[test]
    fn extract_identity_prefers_principal_id() {
        let payload = "eyJwcmluY2lwYWxfaWQiOiJwLTEiLCJlbWFpbCI6ImFAYi5jIn0";
        let tokens = OAuthTokenResponse {
            access_token: format!("h.{payload}.s"),
            refresh_token: None,
            id_token: None,
            expires_in: Some(3600),
            error: None,
            error_description: None,
        };
        let (id, email) = extract_identity(&tokens);
        assert_eq!(id.as_deref(), Some("p-1"));
        assert_eq!(email.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn parse_interval_defends_against_garbage() {
        assert_eq!(parse_interval(Some(&Value::String("NaN".into()))), 5);
        assert_eq!(parse_interval(Some(&Value::Number((-3).into()))), 5);
        assert_eq!(
            parse_interval(Some(&Value::Number(serde_json::Number::from(7u64)))),
            7
        );
    }

    #[tokio::test]
    async fn poll_unknown_session_returns_error() {
        let mgr = XaiOAuthManager::new();
        let poll = mgr.poll_device_login("missing").await.unwrap();
        assert_eq!(poll.status, "error");
        assert!(poll.tokens.is_none());
    }

    #[tokio::test]
    async fn refresh_xai_tokens_rejects_empty_refresh() {
        let err = refresh_xai_tokens("   ").await.unwrap_err();
        assert!(err.contains("refresh_token"), "{err}");
    }

    #[test]
    fn urlencoding_encodes_refresh_token_specials() {
        let encoded = urlencoding_encode("a+b/c=");
        assert_eq!(encoded, "a%2Bb%2Fc%3D");
    }
}
