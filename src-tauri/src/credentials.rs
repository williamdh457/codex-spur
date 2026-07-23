use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    /// Explicit rename: plain `snake_case` turns `OAuth` into `o_auth` (wrong).
    #[serde(rename = "oauth", alias = "o_auth")]
    OAuth,
    /// Explicit rename: plain `snake_case` turns this into `chat_gpt_web_session`.
    #[serde(rename = "chatgpt_web_session", alias = "chat_gpt_web_session")]
    ChatGptWebSession,
    /// Durable Codex Agent Identity (Ed25519 runtime + private key, no OAuth tokens).
    #[serde(rename = "agent_identity", alias = "agentIdentity")]
    AgentIdentity,
}

impl CredentialKind {
    /// Stable DB / IPC string (never use raw serde for `OAuth` — it becomes `o_auth`).
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
            Self::ChatGptWebSession => "chatgpt_web_session",
            Self::AgentIdentity => "agent_identity",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Unknown,
    Refreshable,
    AccessOnly,
    Expired,
    ReauthRequired,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalCredential {
    pub kind: CredentialKind,
    pub state: CredentialState,
    pub provider_id: String,
    pub label: Option<String>,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub fingerprint: String,
    pub refreshable: bool,
    #[serde(skip)]
    pub secret: SecretMaterial,
}

#[derive(Clone, Default)]
pub struct SecretMaterial {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub session_token: Option<String>,
    pub api_key: Option<String>,
    pub agent_runtime_id: Option<String>,
    pub agent_private_key: Option<String>,
    pub task_id: Option<String>,
}

impl std::fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretMaterial(REDACTED)")
    }
}

impl SecretMaterial {
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_slice(bytes)?;
        let string = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        };
        Ok(Self {
            access_token: string("access_token"),
            refresh_token: string("refresh_token"),
            id_token: string("id_token"),
            session_token: string("session_token"),
            api_key: string("api_key"),
            agent_runtime_id: string("agent_runtime_id")
                .or_else(|| string("agentRuntimeId")),
            agent_private_key: string("agent_private_key")
                .or_else(|| string("agentPrivateKey")),
            task_id: string("task_id").or_else(|| string("taskId")),
        })
    }

    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }

    pub fn is_agent_identity(&self) -> bool {
        self.agent_runtime_id
            .as_ref()
            .is_some_and(|v| !v.trim().is_empty())
            && self
                .agent_private_key
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
    }
}

impl CanonicalCredential {
    pub fn assign_provider(mut self, provider_id: &str) -> Self {
        self.provider_id = provider_id.to_string();
        self.fingerprint = fingerprint(
            provider_id,
            self.account_id.as_deref().or(self.email.as_deref()),
            self.secret
                .agent_runtime_id
                .as_deref()
                .or(self.secret.access_token.as_deref())
                .or(self.secret.api_key.as_deref())
                .or(self.secret.session_token.as_deref()),
        );
        self
    }

    pub fn summary(&self) -> CredentialImportSummary {
        CredentialImportSummary {
            kind: self.kind,
            state: self.state,
            provider_id: self.provider_id.clone(),
            label: self.label.clone(),
            masked_email: self.email.as_deref().map(mask_identity),
            masked_account_id: self.account_id.as_deref().map(mask_identity),
            expires_at: self.expires_at,
            fingerprint_prefix: self.fingerprint.chars().take(12).collect(),
            refreshable: self.kind == CredentialKind::AgentIdentity
                || (self.refreshable && self.secret.has_refresh_token()),
        }
    }
}

fn mask_identity(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        return "••••".into();
    }
    let prefix: String = chars.iter().take(2).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}••••{suffix}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialImportSummary {
    pub kind: CredentialKind,
    pub state: CredentialState,
    pub provider_id: String,
    pub label: Option<String>,
    pub masked_email: Option<String>,
    pub masked_account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub fingerprint_prefix: String,
    pub refreshable: bool,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("JSON root must be an object or array")]
    InvalidRoot,
    #[error("account object does not contain a usable credential")]
    MissingCredential,
    #[error("this looks like provider config JSON (base_url/api_key/models), not account JSON")]
    ProviderConfigNotAccount,
    #[error("agent identity private key is invalid")]
    InvalidAgentPrivateKey,
}

pub fn parse_json_import(input: &str) -> Result<Vec<CanonicalCredential>, ImportError> {
    let value: Value = serde_json::from_str(input).map_err(|_| ImportError::InvalidRoot)?;
    if looks_like_provider_config_only(&value) {
        return Err(ImportError::ProviderConfigNotAccount);
    }
    let objects = collect_objects(&value);
    if objects.is_empty() {
        return Err(ImportError::InvalidRoot);
    }
    objects
        .into_iter()
        .filter_map(normalize_object)
        .collect::<Result<Vec<_>, _>>()
}

/// ChatGPT `/api/auth/session` dump (optionally with WARNING_BANNER).
pub fn parse_session_import(input: &str) -> Result<CanonicalCredential, ImportError> {
    let value: Value = serde_json::from_str(input).map_err(|_| ImportError::InvalidRoot)?;
    let object = value.as_object().ok_or(ImportError::InvalidRoot)?;
    // Prefer nested session surface if present.
    let access = object
        .get("accessToken")
        .or_else(|| object.get("access_token"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or(ImportError::MissingCredential)?;
    let email = object
        .get("user")
        .and_then(|u| u.get("email"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let account_id = object
        .get("account")
        .and_then(|a| a.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            crate::openai_oauth::chatgpt_account_id_from_token(&access)
        });
    let label = object
        .get("user")
        .and_then(|u| u.get("name"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let session_token = object
        .get("sessionToken")
        .or_else(|| object.get("session_token"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let expires_at = object
        .get("expires")
        .and_then(parse_expires_value)
        .or_else(|| crate::openai_oauth::jwt_exp_unix(&access));
    Ok(CanonicalCredential {
        kind: CredentialKind::ChatGptWebSession,
        state: CredentialState::AccessOnly,
        provider_id: "openai".into(),
        label,
        email,
        account_id,
        expires_at,
        fingerprint: String::new(),
        refreshable: false,
        secret: SecretMaterial {
            access_token: Some(access),
            session_token,
            ..SecretMaterial::default()
        },
    })
}

fn looks_like_provider_config_only(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    let has_base = map.contains_key("base_url") || map.contains_key("baseUrl");
    let has_models = map.contains_key("models");
    let has_account_shape = [
        "access_token",
        "accessToken",
        "refresh_token",
        "session_token",
        "sessionToken",
        "accounts",
        "agent_identity",
        "agentIdentity",
        "auth_mode",
        "authMode",
        "tokens",
    ]
    .iter()
    .any(|k| map.contains_key(*k));
    has_base && has_models && !has_account_shape
}

fn collect_objects(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().flat_map(collect_objects).collect(),
        Value::Object(map) if map.contains_key("accounts") => {
            map.get("accounts").map(collect_objects).unwrap_or_default()
        }
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    }
}

fn parse_agent_identity_object(
    object: &serde_json::Map<String, Value>,
) -> Option<Result<CanonicalCredential, ImportError>> {
    let auth_mode = object
        .get("auth_mode")
        .or_else(|| object.get("authMode"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let agent_map = object
        .get("agent_identity")
        .or_else(|| object.get("agentIdentity"))
        .and_then(Value::as_object);
    let is_mode = auth_mode.eq_ignore_ascii_case("agentidentity");
    if !is_mode && agent_map.is_none() {
        return None;
    }
    let surface = agent_map.unwrap_or(object);
    let runtime = surface
        .get("agent_runtime_id")
        .or_else(|| surface.get("agentRuntimeId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())?
        .to_string();
    let private_key = surface
        .get("agent_private_key")
        .or_else(|| surface.get("agentPrivateKey"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())?
        .to_string();
    if crate::openai_agent_identity::validate_agent_private_key(&private_key).is_err() {
        return Some(Err(ImportError::InvalidAgentPrivateKey));
    }
    let account_id = surface
        .get("account_id")
        .or_else(|| surface.get("accountId"))
        .or_else(|| surface.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            object
                .get("account_id")
                .or_else(|| object.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
    let email = surface
        .get("email")
        .or_else(|| object.get("email"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let label = object
        .get("label")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let task_id = surface
        .get("task_id")
        .or_else(|| surface.get("taskId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned);
    // Optional usage tokens (Sub2API dual-form exports / re-imports).
    let pick = |map: &serde_json::Map<String, Value>, snake: &str, camel: &str| {
        map.get(snake)
            .or_else(|| map.get(camel))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    let access_token = pick(surface, "access_token", "accessToken")
        .or_else(|| pick(object, "access_token", "accessToken"))
        .or_else(|| {
            object
                .get("tokens")
                .and_then(Value::as_object)
                .and_then(|t| pick(t, "access_token", "accessToken"))
        });
    let refresh_token = pick(surface, "refresh_token", "refreshToken")
        .or_else(|| pick(object, "refresh_token", "refreshToken"))
        .or_else(|| {
            object
                .get("tokens")
                .and_then(Value::as_object)
                .and_then(|t| pick(t, "refresh_token", "refreshToken"))
        });
    let id_token = pick(surface, "id_token", "idToken")
        .or_else(|| pick(object, "id_token", "idToken"))
        .or_else(|| {
            object
                .get("tokens")
                .and_then(Value::as_object)
                .and_then(|t| pick(t, "id_token", "idToken"))
        });
    let session_token = pick(surface, "session_token", "sessionToken")
        .or_else(|| pick(object, "session_token", "sessionToken"));
    Some(Ok(CanonicalCredential {
        kind: CredentialKind::AgentIdentity,
        state: CredentialState::Refreshable,
        provider_id: "openai".into(),
        label,
        email,
        account_id,
        expires_at: None,
        fingerprint: String::new(),
        refreshable: true,
        secret: SecretMaterial {
            access_token,
            refresh_token,
            id_token,
            session_token,
            agent_runtime_id: Some(runtime),
            agent_private_key: Some(private_key),
            task_id,
            ..SecretMaterial::default()
        },
    }))
}

/// Resolve the object that holds token / secret fields.
/// - Codex Tools: nested native auth under `authJson` / `auth_json`
/// - Sub2API exports: OAuth secrets under `credentials`
fn auth_surface(object: &serde_json::Map<String, Value>) -> &serde_json::Map<String, Value> {
    object
        .get("authJson")
        .or_else(|| object.get("auth_json"))
        .or_else(|| object.get("credentials"))
        .and_then(Value::as_object)
        .unwrap_or(object)
}

fn nested_tokens_map<'a>(
    auth: &'a serde_json::Map<String, Value>,
    object: &'a serde_json::Map<String, Value>,
) -> Option<&'a serde_json::Map<String, Value>> {
    auth.get("tokens")
        .and_then(Value::as_object)
        .or_else(|| object.get("tokens").and_then(Value::as_object))
}

fn field_string(
    auth: &serde_json::Map<String, Value>,
    object: &serde_json::Map<String, Value>,
    nested_tokens: Option<&serde_json::Map<String, Value>>,
    name: &str,
) -> Option<String> {
    auth.get(name)
        .or_else(|| object.get(name))
        .or_else(|| nested_tokens.and_then(|tokens| tokens.get(name)))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_object(value: &Value) -> Option<Result<CanonicalCredential, ImportError>> {
    let object = value.as_object()?;
    // Agent Identity auth.json (Codex / Sub2API): no OAuth tokens stored.
    if let Some(agent) = parse_agent_identity_object(object) {
        return Some(agent);
    }
    // Unwrap nested auth containers once (Codex Tools accounts export), then
    // reuse the same field extraction path as native auth.json / flat tokens.
    let auth = auth_surface(object);
    let nested_tokens = nested_tokens_map(auth, object);
    let token = |name: &str| field_string(auth, object, nested_tokens, name);
    let access_token = token("access_token").or_else(|| token("accessToken"));
    let refresh_token = token("refresh_token").or_else(|| token("refreshToken"));
    let id_token = token("id_token").or_else(|| token("idToken"));
    let session_token = token("session_token").or_else(|| token("sessionToken"));
    let api_key = token("api_key")
        .or_else(|| token("apiKey"))
        .or_else(|| token("OPENAI_API_KEY"));
    if access_token.is_none() && api_key.is_none() && session_token.is_none() {
        return Some(Err(ImportError::MissingCredential));
    }

    let provider_id = object
        .get("provider")
        .or_else(|| object.get("platform"))
        .or_else(|| auth.get("provider"))
        .or_else(|| auth.get("platform"))
        .and_then(Value::as_str)
        .unwrap_or("openai")
        .to_lowercase();
    let email = object
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| auth.get("email").and_then(Value::as_str))
        .or_else(|| {
            object
                .get("user")
                .or_else(|| auth.get("user"))
                .and_then(|user| user.get("email"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            nested_tokens
                .and_then(|tokens| tokens.get("id_token"))
                .and_then(|id_token| id_token.get("email"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned);
    let account_id = object
        .get("account_id")
        .and_then(Value::as_str)
        .or_else(|| object.get("accountId").and_then(Value::as_str))
        .or_else(|| object.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| object.get("chatgptAccountId").and_then(Value::as_str))
        .or_else(|| auth.get("account_id").and_then(Value::as_str))
        .or_else(|| auth.get("accountId").and_then(Value::as_str))
        .or_else(|| auth.get("chatgpt_account_id").and_then(Value::as_str))
        .or_else(|| auth.get("chatgptAccountId").and_then(Value::as_str))
        .or_else(|| {
            // Sub2API credential bag often stores chatgpt_account_id under credentials.
            object
                .get("credentials")
                .and_then(Value::as_object)
                .and_then(|creds| {
                    creds
                        .get("chatgpt_account_id")
                        .or_else(|| creds.get("account_id"))
                        .or_else(|| creds.get("organization_id"))
                })
                .and_then(Value::as_str)
        })
        .or_else(|| {
            object
                .get("account")
                .or_else(|| auth.get("account"))
                .and_then(|account| account.get("id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            nested_tokens
                .and_then(|tokens| tokens.get("account_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            nested_tokens
                .and_then(|tokens| tokens.get("accountId"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            nested_tokens
                .and_then(|tokens| tokens.get("id_token"))
                .and_then(|id_token| id_token.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
        // Last resort: decode ChatGPT account id from JWT access/id tokens.
        .or_else(|| {
            access_token
                .as_deref()
                .and_then(crate::openai_oauth::chatgpt_account_id_from_token)
        })
        .or_else(|| {
            id_token
                .as_deref()
                .and_then(crate::openai_oauth::chatgpt_account_id_from_token)
        });
    let expires_at = object
        .get("expires_at")
        .or_else(|| object.get("expiresAt"))
        .or_else(|| object.get("expires"))
        .or_else(|| auth.get("expires_at"))
        .or_else(|| auth.get("expiresAt"))
        .or_else(|| auth.get("expires"))
        .and_then(parse_expires_value)
        // ChatGPT session dumps often omit numeric exp; fall back to JWT `exp`.
        .or_else(|| {
            access_token
                .as_deref()
                .and_then(crate::openai_oauth::jwt_exp_unix)
        });
    let kind = if api_key.is_some() {
        CredentialKind::ApiKey
    } else if session_token.is_some() && refresh_token.is_none() {
        CredentialKind::ChatGptWebSession
    } else {
        CredentialKind::OAuth
    };
    let state = if refresh_token.is_some() {
        CredentialState::Refreshable
    } else if access_token.is_some() || session_token.is_some() {
        CredentialState::AccessOnly
    } else {
        CredentialState::Unknown
    };
    let fingerprint = fingerprint(
        provider_id.as_str(),
        account_id.as_deref().or(email.as_deref()),
        access_token
            .as_deref()
            .or(api_key.as_deref())
            .or(session_token.as_deref()),
    );
    Some(Ok(CanonicalCredential {
        kind,
        state,
        provider_id,
        label: object
            .get("label")
            .or_else(|| object.get("name"))
            .or_else(|| auth.get("label"))
            .or_else(|| auth.get("name"))
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("user")
                    .or_else(|| auth.get("user"))
                    .and_then(|user| user.get("name"))
                    .and_then(Value::as_str)
            })
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned),
        email,
        account_id,
        expires_at,
        fingerprint,
        refreshable: refresh_token.is_some(),
        secret: SecretMaterial {
            access_token,
            refresh_token,
            id_token,
            session_token,
            api_key,
            agent_runtime_id: None,
            agent_private_key: None,
            task_id: None,
        },
    }))
}

fn fingerprint(provider: &str, identity: Option<&str>, secret: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-select-credential-v1\0");
    hasher.update(provider.as_bytes());
    hasher.update([0]);
    hasher.update(identity.unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(secret.unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

/// Accept unix seconds (number/string) or a few ISO-8601 forms used by session dumps.
fn parse_expires_value(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok();
    }
    if let Some(n) = value.as_f64() {
        if n.is_finite() {
            return Some(n as i64);
        }
    }
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Some(n);
    }
    // 2026-10-20T10:04:27.756Z / 2026-10-20T10:04:27Z
    let normalized = raw.trim_end_matches('Z').replace('T', " ");
    let date_time = normalized.split('.').next().unwrap_or(normalized.as_str());
    let mut parts = date_time.split([' ', ':', '-']);
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let hour: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minute: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let second: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil date (Howard Hinnant algorithm) → unix seconds (UTC).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_kind_serializes_as_oauth_not_o_auth() {
        assert_eq!(CredentialKind::OAuth.as_db_str(), "oauth");
        assert_eq!(
            CredentialKind::ChatGptWebSession.as_db_str(),
            "chatgpt_web_session"
        );
        assert_eq!(
            serde_json::to_string(&CredentialKind::OAuth).expect("ser"),
            "\"oauth\""
        );
        assert_eq!(
            serde_json::from_str::<CredentialKind>("\"o_auth\"").expect("legacy"),
            CredentialKind::OAuth
        );
        assert_eq!(
            serde_json::from_str::<CredentialKind>("\"oauth\"").expect("canonical"),
            CredentialKind::OAuth
        );
    }

    #[test]
    fn parses_chatgpt_session_dump_camel_case() {
        // Real browser session dumps often start with WARNING_BANNER; paste whole blob as-is.
        let input = r#"{
            "WARNING_BANNER": "DO NOT SHARE — sensitive session material",
            "user": {"email": "a@example.com", "name": "Ada"},
            "account": {"id": "acc-1", "planType": "plus"},
            "accessToken": "access-only-token",
            "sessionToken": "session-only-token",
            "expires": "2026-10-20T10:04:27.756Z",
            "authProvider": "openai",
            "rumViewTags": {"light_account": {"fetched": false}}
        }"#;
        let parsed = parse_json_import(input).expect("imports session dump");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, CredentialKind::ChatGptWebSession);
        assert_eq!(parsed[0].state, CredentialState::AccessOnly);
        assert_eq!(parsed[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(parsed[0].account_id.as_deref(), Some("acc-1"));
        assert_eq!(parsed[0].label.as_deref(), Some("Ada"));
        assert_eq!(parsed[0].secret.access_token.as_deref(), Some("access-only-token"));
        assert_eq!(parsed[0].secret.session_token.as_deref(), Some("session-only-token"));
        assert!(!parsed[0].refreshable);
        assert_eq!(parsed[0].expires_at, Some(1_792_490_667));

        // Dedicated session import path must accept the same blob as access-only
        // (Agent Identity upgrade is best-effort at the command layer, not required to parse).
        let session = parse_session_import(input).expect("parse_session_import accepts dump");
        assert_eq!(session.kind, CredentialKind::ChatGptWebSession);
        assert_eq!(session.state, CredentialState::AccessOnly);
        assert_eq!(session.secret.access_token.as_deref(), Some("access-only-token"));
        assert_eq!(session.secret.session_token.as_deref(), Some("session-only-token"));
        assert!(!session.refreshable);
    }

    #[test]
    fn parse_session_import_requires_access_token() {
        let err = parse_session_import(r#"{"user":{"email":"a@b.c"},"account":{"id":"x"}}"#)
            .expect_err("missing accessToken");
        assert!(matches!(err, ImportError::MissingCredential));
    }

    #[test]
    fn parse_expires_iso_and_unix() {
        assert_eq!(
            parse_expires_value(&serde_json::json!(1_700_000_000)),
            Some(1_700_000_000)
        );
        assert_eq!(
            parse_expires_value(&serde_json::json!("1700000000")),
            Some(1_700_000_000)
        );
        assert_eq!(
            parse_expires_value(&serde_json::json!("2026-10-20T10:04:27.756Z")),
            Some(1_792_490_667)
        );
    }

    #[test]
    fn parses_codex_auth_shape_without_exposing_secret_in_debug() {
        let input = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"access","refresh_token":"refresh","account_id":"acct"}}"#;
        let parsed = parse_json_import(input).expect("imports");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
        assert!(format!("{:?}", parsed[0].secret).contains("REDACTED"));
        assert!(!parsed[0].fingerprint.contains("access"));
    }

    #[test]
    fn parses_sub2api_accounts_array() {
        let input = r#"{"accounts":[{"access_token":"access","provider":"openai","email":"a@example.com"}]}"#;
        let parsed = parse_json_import(input).expect("imports");
        assert_eq!(parsed[0].kind, CredentialKind::OAuth);
        assert_eq!(parsed[0].state, CredentialState::AccessOnly);
    }

    #[test]
    fn parses_native_codex_auth_json_shape() {
        let items = parse_json_import(
            r#"{
            "OPENAI_API_KEY": "sk-test",
            "tokens": {"access_token": "access", "refresh_token": "refresh", "account_id": "acct"}
        }"#,
        )
        .expect("native auth parses");
        assert_eq!(items[0].provider_id, "openai");
        assert_eq!(items[0].account_id.as_deref(), Some("acct"));
        assert_eq!(items[0].secret.api_key.as_deref(), Some("sk-test"));
        assert_eq!(items[0].state, CredentialState::Refreshable);
        assert!(items[0].refreshable);
        assert!(items[0].secret.has_refresh_token());
    }

    #[test]
    fn parses_codex_tools_accounts_with_nested_auth_json() {
        // Codex Tools accounts export nests real Codex auth under authJson.tokens.
        // This was the failure mode behind: "account object does not contain a usable credential".
        let input = r#"{
            "accounts": [
                {
                    "email": "user@example.com",
                    "label": "primary",
                    "authJson": {
                        "auth_mode": "chatgpt",
                        "tokens": {
                            "access_token": "access",
                            "refresh_token": "refresh",
                            "account_id": "acct-nested"
                        }
                    }
                }
            ]
        }"#;
        let parsed = parse_json_import(input).expect("nested authJson imports");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, CredentialKind::OAuth);
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
        assert_eq!(parsed[0].email.as_deref(), Some("user@example.com"));
        assert_eq!(parsed[0].label.as_deref(), Some("primary"));
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-nested"));
        assert_eq!(parsed[0].secret.access_token.as_deref(), Some("access"));
        assert_eq!(parsed[0].secret.refresh_token.as_deref(), Some("refresh"));
        assert!(parsed[0].refreshable);
        assert!(parsed[0].secret.has_refresh_token());
        // Must not surface the missing-credential error for this shape.
        let err = ImportError::MissingCredential.to_string();
        assert!(err.contains("usable credential"));
    }

    #[test]
    fn parses_auth_json_snake_case_wrapper() {
        let input = r#"{
            "accounts": [{
                "auth_json": {
                    "tokens": {
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "account_id": "acct-snake"
                    }
                }
            }]
        }"#;
        let parsed = parse_json_import(input).expect("auth_json wrapper imports");
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-snake"));
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
    }

    #[test]
    fn parses_flat_api_key_object() {
        let parsed =
            parse_json_import(r#"{"api_key":"sk-live-test","provider":"openai"}"#).expect("api key");
        assert_eq!(parsed[0].kind, CredentialKind::ApiKey);
        assert_eq!(parsed[0].secret.api_key.as_deref(), Some("sk-live-test"));
    }

    #[test]
    fn rejects_metadata_only_account_without_secrets() {
        let err = parse_json_import(r#"{"accounts":[{"email":"a@example.com","label":"empty"}]}"#)
            .expect_err("metadata-only must fail");
        assert!(matches!(err, ImportError::MissingCredential));
        assert_eq!(
            err.to_string(),
            "account object does not contain a usable credential"
        );
    }

    #[test]
    fn parses_sub2api_chatgpt_account_id_from_credentials_bag() {
        let input = r#"{
            "accounts": [{
                "platform": "openai",
                "credentials": {
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "chatgpt_account_id": "acct-from-creds"
                }
            }]
        }"#;
        let parsed = parse_json_import(input).expect("sub2api chatgpt_account_id");
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-from-creds"));
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
    }

    #[test]
    fn parses_sub2api_nested_credentials_export() {
        // Sub2API plus-usable-style export: secrets live under accounts[].credentials,
        // not top-level and not under authJson/tokens.
        let input = r#"{
            "type": "sub2api-data",
            "version": 1,
            "accounts": [
                {
                    "name": "acct-one",
                    "platform": "openai",
                    "type": "oauth",
                    "credentials": {
                        "email": "a@example.com",
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "id_token": "idtok",
                        "account_id": "acct-1",
                        "chatgpt_account_id": "acct-1",
                        "expires_at": 1785124169
                    },
                    "extra": {"source": "go-pool"}
                },
                {
                    "name": "acct-two",
                    "platform": "openai",
                    "type": "oauth",
                    "credentials": {
                        "email": "b@example.com",
                        "access_token": "access-2",
                        "refresh_token": "refresh-2",
                        "chatgpt_account_id": "acct-2"
                    }
                }
            ]
        }"#;
        let parsed = parse_json_import(input).expect("sub2api nested credentials import");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].kind, CredentialKind::OAuth);
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
        assert_eq!(parsed[0].provider_id, "openai");
        assert_eq!(parsed[0].label.as_deref(), Some("acct-one"));
        assert_eq!(parsed[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-1"));
        assert_eq!(parsed[0].expires_at, Some(1785124169));
        assert_eq!(parsed[0].secret.access_token.as_deref(), Some("access"));
        assert_eq!(parsed[0].secret.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(parsed[0].secret.id_token.as_deref(), Some("idtok"));
        assert!(parsed[0].refreshable);
        assert!(parsed[0].secret.has_refresh_token());
        assert_eq!(parsed[1].account_id.as_deref(), Some("acct-2"));
        assert_eq!(parsed[1].email.as_deref(), Some("b@example.com"));
        assert_eq!(parsed[1].secret.access_token.as_deref(), Some("access-2"));
        assert!(format!("{:?}", parsed[0].secret).contains("REDACTED"));
        assert!(!format!("{:?}", parsed[0].secret).contains("access"));
    }

    #[test]
    fn parses_sub2api_credentials_without_top_level_identity() {
        // Identity only under credentials; name/platform only on the account shell.
        let input = r#"{
            "type":"sub2api-data",
            "accounts":[{
                "name":"n",
                "platform":"openai",
                "type":"oauth",
                "credentials":{
                    "email":"a@example.com",
                    "access_token":"access",
                    "refresh_token":"refresh",
                    "account_id":"acct-1"
                }
            }]
        }"#;
        let parsed = parse_json_import(input).expect("imports");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].state, CredentialState::Refreshable);
        assert_eq!(parsed[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-1"));
        assert_eq!(parsed[0].label.as_deref(), Some("n"));
    }

    #[test]
    fn parses_agent_identity_with_usage_tokens() {
        let material = crate::openai_agent_identity::generate_agent_key_material().expect("key");
        let input = format!(
            r#"{{
                "auth_mode": "agentIdentity",
                "email": "user@example.com",
                "account_id": "acct-agent-1",
                "label": "Primary",
                "agent_runtime_id": "runtime-xyz",
                "agent_private_key": "{}",
                "access_token": "usage-access",
                "refresh_token": "usage-refresh"
            }}"#,
            material.private_key_pkcs8_base64
        );
        let parsed = parse_json_import(&input).expect("agent + tokens");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, CredentialKind::AgentIdentity);
        assert_eq!(parsed[0].account_id.as_deref(), Some("acct-agent-1"));
        assert_eq!(parsed[0].secret.access_token.as_deref(), Some("usage-access"));
        assert_eq!(parsed[0].secret.refresh_token.as_deref(), Some("usage-refresh"));
        assert_eq!(parsed[0].secret.agent_runtime_id.as_deref(), Some("runtime-xyz"));
        assert!(parsed[0].secret.is_agent_identity());
    }
}
