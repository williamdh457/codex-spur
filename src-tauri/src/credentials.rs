use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ApiKey,
    OAuth,
    ChatGptWebSession,
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
        })
    }

    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }
}

impl CanonicalCredential {
    pub fn assign_provider(mut self, provider_id: &str) -> Self {
        self.provider_id = provider_id.to_string();
        self.fingerprint = fingerprint(
            provider_id,
            self.account_id.as_deref().or(self.email.as_deref()),
            self.secret
                .access_token
                .as_deref()
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
            refreshable: self.refreshable && self.secret.has_refresh_token(),
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
}

pub fn parse_json_import(input: &str) -> Result<Vec<CanonicalCredential>, ImportError> {
    let value: Value = serde_json::from_str(input).map_err(|_| ImportError::InvalidRoot)?;
    let objects = collect_objects(&value);
    if objects.is_empty() {
        return Err(ImportError::InvalidRoot);
    }
    objects
        .into_iter()
        .filter_map(normalize_object)
        .collect::<Result<Vec<_>, _>>()
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

fn normalize_object(value: &Value) -> Option<Result<CanonicalCredential, ImportError>> {
    let object = value.as_object()?;
    let nested_tokens = object.get("tokens").and_then(Value::as_object);
    let token = |name: &str| {
        object
            .get(name)
            .or_else(|| nested_tokens.and_then(|tokens| tokens.get(name)))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned)
    };
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
        .and_then(Value::as_str)
        .unwrap_or("openai")
        .to_lowercase();
    let email = object
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("user")
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
        .or_else(|| {
            object
                .get("account")
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
        .map(ToOwned::to_owned);
    let expires_at = object
        .get("expires_at")
        .and_then(Value::as_i64)
        .or_else(|| object.get("expiresAt").and_then(Value::as_i64))
        .or_else(|| object.get("expires").and_then(Value::as_i64));
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
            .and_then(Value::as_str)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
