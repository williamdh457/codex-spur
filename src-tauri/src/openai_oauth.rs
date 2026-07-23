//! OpenAI ChatGPT / Codex subscription OAuth.
//!
//! Supports:
//! 1. Browser localhost PKCE (primary UI path — matches official Codex / codex-tools)
//! 2. Device-code login (kept as fallback)
//! 3. Refresh-token renewal for vault-stored sessions
//!
//! Secrets stay in Rust; the frontend only receives non-sensitive status.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// Public Codex CLI OAuth client id.
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_AUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OAUTH_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const DEVICE_VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const OAUTH_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
/// Scope used when refreshing tokens (aligned with OpenAI OAuth / Codex clients).
const REFRESH_SCOPE: &str = "openid profile email";
const OAUTH_ORIGINATOR: &str = "codex_cli_rs";
pub const DEFAULT_OAUTH_REDIRECT_PORT: u16 = 1455;
const BROWSER_LOGIN_TIMEOUT_SECS: u64 = 900;
const USER_AGENT: &str = "codex_cli_rs/0.144.1";
/// Refresh access tokens this many seconds before JWT/exp expiry.
const REFRESH_LEAD_SECS: i64 = 120;
/// ChatGPT multi-account check — map keys are ChatGPT account / workspace ids.
const ACCOUNTS_CHECK_URL: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
/// Codex personal-access-token whoami (tokens starting with `at-`).
const CODEX_PAT_WHOAMI_URL: &str =
    "https://auth.openai.com/api/accounts/v1/user-auth-credential/whoami";

// ── Shared public types ──────────────────────────────────────────────────────

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLoginStart {
    pub auth_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone)]
pub struct PendingBrowserLogin {
    pub redirect_uri: String,
    pub state: String,
    pub code_verifier: String,
    pub expires_at: Instant,
    pub display_name: Option<String>,
}

// ── Internal response shapes ─────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct OpenAiOAuthManager {
    pending: Arc<RwLock<HashMap<String, PendingDevice>>>,
    browser: Arc<RwLock<Option<PendingBrowserLogin>>>,
}

#[derive(Clone)]
struct PendingDevice {
    user_code: String,
    expires_at: Instant,
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

// ── Manager ──────────────────────────────────────────────────────────────────

impl OpenAiOAuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Device code (fallback) ───────────────────────────────────────────────

    pub async fn start_device_login(&self) -> Result<DeviceLoginStart, String> {
        let client = auth_http_client()?;
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

        let client = auth_http_client()?;
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
        let tokens = exchange_code_for_tokens(
            &client,
            &success.authorization_code,
            &success.code_verifier,
            DEVICE_REDIRECT_URI,
        )
        .await?;
        {
            let mut pending = self.pending.write().await;
            pending.remove(device_code);
        }
        let tokens = finalize_login_tokens(tokens_from_oauth(tokens)?).await?;
        Ok(DeviceLoginPoll {
            status: "success".into(),
            tokens: Some(tokens),
            message: None,
        })
    }

    pub async fn cancel_device_login(&self, device_code: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(device_code);
    }

    // ── Browser PKCE (primary) ───────────────────────────────────────────────

    /// Prepare a browser login session. Caller binds the callback port first and
    /// passes the actual port so `redirect_uri` matches the listener.
    pub async fn prepare_browser_login(
        &self,
        redirect_port: u16,
        display_name: Option<String>,
    ) -> Result<(BrowserLoginStart, PendingBrowserLogin), String> {
        let state = uuid::Uuid::new_v4().simple().to_string();
        let code_verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let code_challenge = base64url_encode_nopad(&Sha256::digest(code_verifier.as_bytes()));
        let redirect_uri = format!("http://localhost:{redirect_port}/auth/callback");

        let mut url = url::Url::parse(OAUTH_AUTHORIZE_URL)
            .map_err(|e| format!("构造授权 URL 失败：{e}"))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CODEX_CLIENT_ID)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", OAUTH_SCOPE)
            .append_pair("state", &state)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", OAUTH_ORIGINATOR);

        let pending = PendingBrowserLogin {
            redirect_uri: redirect_uri.clone(),
            state,
            code_verifier,
            expires_at: Instant::now() + Duration::from_secs(BROWSER_LOGIN_TIMEOUT_SECS),
            display_name,
        };
        {
            let mut guard = self.browser.write().await;
            *guard = Some(pending.clone());
        }
        Ok((
            BrowserLoginStart {
                auth_url: url.to_string(),
                redirect_uri,
            },
            pending,
        ))
    }

    pub async fn peek_pending_browser(&self) -> Option<PendingBrowserLogin> {
        self.browser.read().await.clone()
    }

    pub async fn clear_browser_login(&self) {
        let mut guard = self.browser.write().await;
        *guard = None;
    }

    pub async fn clear_browser_if_state_matches(&self, state: &str) {
        let mut guard = self.browser.write().await;
        if guard.as_ref().is_some_and(|p| p.state == state) {
            *guard = None;
        }
    }

    /// Exchange a browser callback URL for tokens using the current pending session.
    pub async fn complete_browser_callback(
        &self,
        callback_url: &str,
    ) -> Result<DeviceLoginTokens, String> {
        let pending = self
            .peek_pending_browser()
            .await
            .ok_or_else(|| "请先打开授权页面".to_string())?;
        if pending.expires_at <= Instant::now() {
            self.clear_browser_login().await;
            return Err("登录已过期，请重新开始".into());
        }
        let tokens = exchange_from_callback_url(callback_url, &pending).await?;
        self.clear_browser_if_state_matches(&pending.state).await;
        Ok(tokens)
    }
}

// ── Token exchange / refresh ─────────────────────────────────────────────────

async fn exchange_from_callback_url(
    callback_url: &str,
    pending: &PendingBrowserLogin,
) -> Result<DeviceLoginTokens, String> {
    let parsed = parse_oauth_callback_url(callback_url)?;
    let params: HashMap<String, String> = parsed
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error.as_str());
        return Err(format!("授权失败：{description}"));
    }
    let state = params
        .get("state")
        .ok_or_else(|| "回调链接缺少 state 参数".to_string())?;
    if state != &pending.state {
        return Err("回调链接 state 不匹配，请重新开始登录".into());
    }
    let code = params
        .get("code")
        .ok_or_else(|| "回调链接缺少 code 参数".to_string())?;

    let client = auth_http_client()?;
    let oauth = exchange_code_for_tokens(
        &client,
        code,
        &pending.code_verifier,
        &pending.redirect_uri,
    )
    .await?;
    finalize_login_tokens(tokens_from_oauth(oauth)?).await
}

async fn exchange_code_for_tokens(
    client: &reqwest::Client,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenResponse, String> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding_encode(code),
        urlencoding_encode(redirect_uri),
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

/// Refresh ChatGPT OAuth tokens. Returns updated access/id/refresh + expires_at.
///
/// `account_id` may be empty when JWT claims omit it (common for partial JSON
/// imports). Callers should run [`ensure_chatgpt_account_id`] before quota /
/// upstream requests that need the ChatGPT-Account-Id header.
pub async fn refresh_chatgpt_tokens(
    refresh_token: &str,
    id_token: Option<&str>,
) -> Result<DeviceLoginTokens, String> {
    let refresh_token = refresh_token.trim();
    if refresh_token.is_empty() {
        return Err("缺少 refresh_token".into());
    }
    let mut client_id = CODEX_CLIENT_ID.to_string();
    // Prefer client_id from claims when present (codex-tools pattern).
    if let Some(claims) = id_token.and_then(parse_jwt_payload) {
        if let Some(cid) = extract_client_id_from_claims(&claims) {
            client_id = cid;
        }
    }
    let form = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}&scope={}",
        urlencoding_encode(refresh_token),
        urlencoding_encode(&client_id),
        urlencoding_encode(REFRESH_SCOPE),
    );

    let client = auth_http_client()?;
    let response = client
        .post(OAUTH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .map_err(|e| format!("刷新 token 失败：{e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let snippet = text.chars().take(200).collect::<String>();
        return Err(format!("刷新 token 失败（{status}）：{snippet}"));
    }
    let oauth: OAuthTokenResponse = response
        .json()
        .await
        .map_err(|e| format!("解析刷新响应失败：{e}"))?;
    // Keep previous refresh_token if server omits rotation.
    let mut tokens = tokens_from_oauth(oauth)?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    // Best-effort network recovery so refresh always tries to restore account_id.
    if tokens.account_id.trim().is_empty() {
        if let Ok(recovered) = recover_chatgpt_account_id_network(&tokens.access_token).await {
            tokens.account_id = recovered;
        }
    }
    Ok(tokens)
}

/// True when access token should be refreshed before use.
pub fn access_token_needs_refresh(access_token: &str, stored_expires_at: Option<i64>) -> bool {
    let now = chrono_now_secs();
    if let Some(exp) = stored_expires_at {
        if now + REFRESH_LEAD_SECS >= exp {
            return true;
        }
    }
    if let Some(claims) = parse_jwt_payload(access_token) {
        if let Some(exp) = claims.get("exp").and_then(Value::as_i64) {
            return now + REFRESH_LEAD_SECS >= exp;
        }
    }
    // No expiry info — do not force refresh every request.
    false
}

pub fn jwt_exp_unix(token: &str) -> Option<i64> {
    parse_jwt_payload(token)?
        .get("exp")
        .and_then(Value::as_i64)
}

/// Extract ChatGPT account id from an access or id token JWT (no network).
pub fn chatgpt_account_id_from_token(token: &str) -> Option<String> {
    let claims = parse_jwt_payload(token)?;
    account_id_from_claims(&claims)
}

/// Resolve ChatGPT account id for quota / upstream headers.
///
/// Order:
/// 1. already-known stored value;
/// 2. JWT claims on access / id tokens;
/// 3. network recovery (`whoami` for `at-*`, else `accounts/check`).
pub async fn ensure_chatgpt_account_id(
    access_token: &str,
    id_token: Option<&str>,
    known: Option<&str>,
) -> Result<String, String> {
    if let Some(id) = known.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(id.to_string());
    }
    if let Some(id) = chatgpt_account_id_from_token(access_token) {
        return Ok(id);
    }
    if let Some(id) = id_token.and_then(chatgpt_account_id_from_token) {
        return Ok(id);
    }
    recover_chatgpt_account_id_network(access_token).await
}

/// Network recovery when JWT claims omit chatgpt_account_id.
pub async fn recover_chatgpt_account_id_network(access_token: &str) -> Result<String, String> {
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err("缺少 access_token，无法补拉 account_id".into());
    }
    // Codex personal access tokens expose whoami.
    if access_token.starts_with("at-") {
        match fetch_account_id_from_whoami(access_token).await {
            Ok(id) => return Ok(id),
            Err(error) => {
                tracing::debug!(error = %error, "codex whoami account_id recovery failed");
            }
        }
    }
    let preferred = jwt_account_hint(access_token);
    fetch_account_id_from_accounts_check(access_token, preferred.as_deref()).await
}

/// Pure selection of account id from a `/backend-api/accounts/check` JSON body.
///
/// The `accounts` object is keyed by ChatGPT account / workspace ids. Preference:
/// preferred hint → `is_default` → non-free plan → first usable entry.
pub fn select_account_id_from_accounts_check(
    body: &Value,
    preferred: Option<&str>,
) -> Option<String> {
    let accounts = body.get("accounts")?.as_object()?;
    if accounts.is_empty() {
        return None;
    }

    let preferred = preferred.map(str::trim).filter(|value| !value.is_empty());
    if let Some(pref) = preferred {
        if let Some(acct) = accounts.get(pref) {
            if is_usable_account_candidate(acct) {
                return Some(pref.to_string());
            }
        }
    }

    let mut default_id: Option<String> = None;
    let mut paid_id: Option<String> = None;
    let mut any_id: Option<String> = None;

    for (id, acct) in accounts {
        if id.trim().is_empty() || !is_usable_account_candidate(acct) {
            continue;
        }
        if any_id.is_none() {
            any_id = Some(id.clone());
        }
        let plan = plan_type_from_account_entry(acct).unwrap_or_default();
        let is_default = acct
            .get("account")
            .and_then(|account| account.get("is_default"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_default && default_id.is_none() {
            default_id = Some(id.clone());
        }
        if !plan.eq_ignore_ascii_case("free") && paid_id.is_none() {
            paid_id = Some(id.clone());
        }
    }

    default_id.or(paid_id).or(any_id)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn tokens_from_oauth(tokens: OAuthTokenResponse) -> Result<DeviceLoginTokens, String> {
    let (account_id, email) = extract_identity(&tokens);
    // Do not hard-fail when claims omit account_id — network recovery can fill it.
    let account_id = account_id.unwrap_or_default();
    let expires_at = tokens
        .expires_in
        .map(|secs| chrono_now_secs().saturating_add(secs as i64))
        .or_else(|| jwt_exp_unix(&tokens.access_token));
    Ok(DeviceLoginTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        email,
        expires_at,
    })
}

/// After OAuth login, require a usable account_id (JWT or network).
async fn finalize_login_tokens(mut tokens: DeviceLoginTokens) -> Result<DeviceLoginTokens, String> {
    if tokens.account_id.trim().is_empty() {
        tokens.account_id = ensure_chatgpt_account_id(
            &tokens.access_token,
            tokens.id_token.as_deref(),
            None,
        )
        .await
        .map_err(|error| {
            format!("无法从 token 解析 ChatGPT account_id，网络补拉也失败：{error}")
        })?;
    }
    Ok(tokens)
}

async fn fetch_account_id_from_whoami(access_token: &str) -> Result<String, String> {
    let client = auth_http_client()?;
    let response = client
        .get(CODEX_PAT_WHOAMI_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("originator", OAUTH_ORIGINATOR)
        .send()
        .await
        .map_err(|e| format!("whoami 请求失败：{e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let snippet = text.chars().take(160).collect::<String>();
        return Err(format!("whoami 失败（{status}）：{snippet}"));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("whoami 响应不是 JSON：{e}"))?;
    body.get("chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "whoami 响应缺少 chatgpt_account_id".to_string())
}

async fn fetch_account_id_from_accounts_check(
    access_token: &str,
    preferred: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(ACCOUNTS_CHECK_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .send()
        .await
        .map_err(|e| format!("accounts/check 请求失败：{e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let snippet = text.chars().take(160).collect::<String>();
        return Err(format!("accounts/check 失败（{status}）：{snippet}"));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("accounts/check 响应不是 JSON：{e}"))?;
    select_account_id_from_accounts_check(&body, preferred)
        .ok_or_else(|| "accounts/check 未返回可用 ChatGPT account_id".to_string())
}

fn account_id_from_claims(claims: &Value) -> Option<String> {
    let auth = claims.get("https://api.openai.com/auth");
    auth.and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .or_else(|| claims.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| claims.get("account_id").and_then(Value::as_str))
        .or_else(|| auth.and_then(|a| a.get("account_id")).and_then(Value::as_str))
        .or_else(|| organization_id_from_auth(auth))
        // poid is the workspace/org id used as accounts/check map key for many tokens.
        .or_else(|| auth.and_then(|a| a.get("poid")).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn organization_id_from_auth(auth: Option<&Value>) -> Option<&str> {
    let orgs = auth?.get("organizations")?.as_array()?;
    let default_org = orgs.iter().find_map(|org| {
        let is_default = org
            .get("is_default")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_default {
            org.get("id").and_then(Value::as_str)
        } else {
            None
        }
    });
    default_org.or_else(|| orgs.first().and_then(|org| org.get("id").and_then(Value::as_str)))
}

/// Hint used to pick the right entry from multi-account `accounts/check`.
fn jwt_account_hint(token: &str) -> Option<String> {
    let claims = parse_jwt_payload(token)?;
    let auth = claims.get("https://api.openai.com/auth");
    auth.and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .or_else(|| auth.and_then(|a| a.get("poid")).and_then(Value::as_str))
        .or_else(|| organization_id_from_auth(auth))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn is_usable_account_candidate(acct: &Value) -> bool {
    if has_deactivated_marker(acct) {
        return false;
    }
    if acct
        .get("account")
        .is_some_and(has_deactivated_marker)
    {
        return false;
    }
    // Expired entitlement → skip when we can parse expires_at.
    if let Some(expires_at) = entitlement_expires_at(acct) {
        if let Ok(expiry) = chrono_rfc3339_to_unix(&expires_at) {
            if expiry <= chrono_now_secs() {
                return false;
            }
        }
    }
    true
}

fn has_deactivated_marker(value: &Value) -> bool {
    value
        .get("is_deactivated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value
            .get("deactivated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn plan_type_from_account_entry(acct: &Value) -> Option<String> {
    acct.get("account")
        .and_then(|account| account.get("plan_type"))
        .and_then(Value::as_str)
        .or_else(|| {
            acct.get("entitlement")
                .and_then(|entitlement| entitlement.get("subscription_plan"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn entitlement_expires_at(acct: &Value) -> Option<String> {
    acct.get("entitlement")
        .and_then(|entitlement| entitlement.get("expires_at"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn chrono_rfc3339_to_unix(value: &str) -> Result<i64, ()> {
    // Minimal RFC3339 parser for Z / +00:00 forms without pulling chrono.
    // Accepts: 2026-07-20T12:00:00Z or 2026-07-20T12:00:00+00:00
    let trimmed = value.trim();
    let (date_time, offset_secs) = if let Some(rest) = trimmed.strip_suffix('Z') {
        (rest, 0_i64)
    } else if let Some(idx) = trimmed.rfind(['+', '-']) {
        if idx < 10 {
            return Err(());
        }
        let (head, offset) = trimmed.split_at(idx);
        let sign = if offset.starts_with('+') { 1_i64 } else { -1_i64 };
        let offset_body = &offset[1..];
        let parts: Vec<&str> = offset_body.split(':').collect();
        if parts.len() != 2 {
            return Err(());
        }
        let hours: i64 = parts[0].parse().map_err(|_| ())?;
        let mins: i64 = parts[1].parse().map_err(|_| ())?;
        (head, sign * (hours * 3600 + mins * 60))
    } else {
        return Err(());
    };
    let (date, time) = date_time.split_once('T').ok_or(())?;
    let mut d = date.split('-');
    let year: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let month: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let day: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let time = time.split('.').next().unwrap_or(time);
    let mut t = time.split(':');
    let hour: i64 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    let minute: i64 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    let second: i64 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(());
    }
    // Days from civil date (Howard Hinnant algorithm) → Unix seconds.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Ok(days * 86400 + hour * 3600 + minute * 60 + second - offset_secs)
}

fn auth_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

pub fn parse_oauth_callback_url(callback_url: &str) -> Result<url::Url, String> {
    let trimmed = callback_url.trim();
    if trimmed.is_empty() {
        return Err("请粘贴回调链接".into());
    }
    url::Url::parse(trimmed)
        .or_else(|_| url::Url::parse(&format!("http://localhost{trimmed}")))
        .map_err(|e| format!("无法解析回调链接：{e}"))
}

pub fn oauth_success_html() -> String {
    oauth_html_page(
        "登录成功",
        "ChatGPT 账号已授权。可以关闭此页，返回 Codex Spur。",
    )
}

pub fn oauth_error_html(message: &str) -> String {
    let safe = html_escape(message);
    oauth_html_page("登录失败", &safe)
}

fn oauth_html_page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Codex Spur</title>
<style>
  body {{ margin:0; padding:40px 20px; font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;
         background:#0f1419; color:#e7ecf3; }}
  main {{ max-width:480px; margin:0 auto; padding:28px; border-radius:16px;
         background:#1a222d; box-shadow:0 12px 40px rgba(0,0,0,.35); }}
  h1 {{ margin:0 0 12px; font-size:22px; }}
  p {{ margin:0; color:#9aabbd; line-height:1.55; word-break:break-word; }}
</style>
</head>
<body><main><h1>{title}</h1><p>{body}</p></main></body>
</html>"#
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

fn base64url_encode_nopad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
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
                account_id = account_id_from_claims(&claims);
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

fn extract_client_id_from_claims(claims: &Value) -> Option<String> {
    claims
        .get("azp")
        .or_else(|| claims.get("aud"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else if let Some(arr) = v.as_array() {
                arr.first().and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
        .filter(|s| s.starts_with("app_"))
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
        let payload = "eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2MtMSIsImVtYWlsIjoiYUBiLmMifQ";
        let token = format!("xxx.{payload}.yyy");
        let claims = parse_jwt_payload(&token).expect("claims");
        assert_eq!(
            claims.get("chatgpt_account_id").and_then(Value::as_str),
            Some("acc-1")
        );
        assert_eq!(claims.get("email").and_then(Value::as_str), Some("a@b.c"));
    }

    #[test]
    fn pkce_challenge_is_s256_base64url() {
        // RFC 7636 appendix B style: fixed verifier → known shape
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64url_encode_nopad(&Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(!challenge.contains('+') && !challenge.contains('/'));
    }

    #[test]
    fn parse_callback_extracts_query() {
        let url = parse_oauth_callback_url(
            "http://localhost:1455/auth/callback?code=abc&state=xyz",
        )
        .expect("url");
        assert_eq!(url.path(), "/auth/callback");
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params.get("code").map(String::as_str), Some("abc"));
        assert_eq!(params.get("state").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn selects_default_account_from_accounts_check() {
        let body = serde_json::json!({
            "accounts": {
                "org-expired": {
                    "account": { "plan_type": "team", "is_default": true },
                    "entitlement": { "expires_at": "2020-01-01T00:00:00Z" }
                },
                "acc-paid": {
                    "account": { "plan_type": "plus", "is_default": false }
                },
                "acc-free": {
                    "account": { "plan_type": "free", "is_default": false }
                }
            }
        });
        assert_eq!(
            select_account_id_from_accounts_check(&body, None).as_deref(),
            Some("acc-paid")
        );
        assert_eq!(
            select_account_id_from_accounts_check(&body, Some("acc-free")).as_deref(),
            Some("acc-free")
        );
    }

    #[test]
    fn selects_preferred_account_when_usable() {
        let body = serde_json::json!({
            "accounts": {
                "pref": {
                    "account": { "plan_type": "pro", "is_default": false }
                },
                "other": {
                    "account": { "plan_type": "plus", "is_default": true }
                }
            }
        });
        assert_eq!(
            select_account_id_from_accounts_check(&body, Some("pref")).as_deref(),
            Some("pref")
        );
    }

    #[test]
    fn skips_deactivated_accounts_check_entries() {
        let body = serde_json::json!({
            "accounts": {
                "dead": {
                    "account": {
                        "plan_type": "plus",
                        "is_default": true,
                        "is_deactivated": true
                    }
                },
                "live": {
                    "account": { "plan_type": "free" }
                }
            }
        });
        assert_eq!(
            select_account_id_from_accounts_check(&body, Some("dead")).as_deref(),
            Some("live")
        );
    }

    #[test]
    fn extracts_account_id_from_nested_auth_and_org() {
        let payload = base64url_encode_nopad(
            br#"{"https://api.openai.com/auth":{"organizations":[{"id":"org-1","is_default":true}]}}"#,
        );
        let token = format!("x.{payload}.y");
        assert_eq!(
            chatgpt_account_id_from_token(&token).as_deref(),
            Some("org-1")
        );
    }

    #[test]
    fn access_token_needs_refresh_respects_exp() {
        // exp far in the future
        let far = chrono_now_secs() + 3600;
        let payload = base64url_encode_nopad(
            format!(r#"{{"exp":{far}}}"#).as_bytes(),
        );
        let token = format!("x.{payload}.y");
        assert!(!access_token_needs_refresh(&token, Some(far)));

        let past = chrono_now_secs() - 10;
        let payload = base64url_encode_nopad(
            format!(r#"{{"exp":{past}}}"#).as_bytes(),
        );
        let token = format!("x.{payload}.y");
        assert!(access_token_needs_refresh(&token, Some(past)));
    }

    #[test]
    fn prepare_browser_login_url_shape() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mgr = OpenAiOAuthManager::new();
            let (start, pending) = mgr
                .prepare_browser_login(1455, Some("work".into()))
                .await
                .expect("prepare");
            assert!(start.auth_url.contains("auth.openai.com/oauth/authorize"));
            assert!(start.auth_url.contains("code_challenge_method=S256"));
            assert!(start.auth_url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
            assert!(start
                .redirect_uri
                .contains("localhost:1455/auth/callback"));
            assert_eq!(pending.redirect_uri, start.redirect_uri);
            assert!(!pending.code_verifier.is_empty());
            assert!(!pending.state.is_empty());
        });
    }
}
