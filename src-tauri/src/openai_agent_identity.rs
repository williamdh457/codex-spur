//! OpenAI Codex Agent Identity: register a durable Ed25519 runtime from a
//! ChatGPT access token, then sign per-request `AgentAssertion` headers.
//!
//! Protocol aligned with OpenAI Codex `codex-rs/agent-identity` (Apache-2.0
//! behavioral reference). Independent implementation — not copied from Sub2API.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use crypto_box::SecretKey as CurveSecretKey;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

use crate::credentials::{
    CanonicalCredential, CredentialKind, CredentialState, SecretMaterial,
};
use crate::openai_oauth::{chatgpt_account_id_from_token, jwt_exp_unix};

const AUTH_API_BASE: &str = "https://auth.openai.com/api/accounts";
const KEY_CONTEXT: &[u8] = b"codex-agent-identity-ed25519-v1";
const REGISTER_TIMEOUT: Duration = Duration::from_secs(20);
const TASK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct AgentKeyMaterial {
    pub private_key_pkcs8_base64: String,
    pub public_key_ssh: String,
}

#[derive(Clone, Debug)]
pub struct AgentIdentityKey {
    pub agent_runtime_id: String,
    pub private_key_pkcs8_base64: String,
    pub task_id: Option<String>,
}

#[derive(Serialize)]
struct RegisterAgentRequest {
    abom: AgentBillOfMaterials,
    agent_public_key: String,
    capabilities: Vec<String>,
    ttl: Option<u64>,
}

#[derive(Serialize)]
struct AgentBillOfMaterials {
    agent_version: String,
    agent_harness_id: String,
    running_location: String,
}

#[derive(Deserialize)]
struct RegisterAgentResponse {
    agent_runtime_id: String,
}

#[derive(Serialize)]
struct RegisterTaskRequest {
    timestamp: String,
    signature: String,
}

#[derive(Deserialize)]
struct RegisterTaskResponse {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default, rename = "taskId")]
    task_id_camel: Option<String>,
    #[serde(default)]
    encrypted_task_id: Option<String>,
    #[serde(default, rename = "encryptedTaskId")]
    encrypted_task_id_camel: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AgentAssertionEnvelope {
    agent_runtime_id: String,
    task_id: String,
    timestamp: String,
    signature: String,
}

pub fn generate_agent_key_material() -> Result<AgentKeyMaterial> {
    let mut seed_material = [0u8; 64];
    getrandom::fill(&mut seed_material).map_err(|e| anyhow!("rng failed: {e}"))?;
    let mut digest = Sha512::new();
    digest.update(KEY_CONTEXT);
    digest.update(seed_material);
    let digest = digest.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&digest[..32]);
    let signing_key = SigningKey::from_bytes(&secret);
    let pkcs8 = signing_key
        .to_pkcs8_der()
        .context("encode agent private key PKCS#8")?;
    Ok(AgentKeyMaterial {
        private_key_pkcs8_base64: B64.encode(pkcs8.as_bytes()),
        public_key_ssh: encode_ssh_ed25519_public_key(&signing_key.verifying_key()),
    })
}

pub fn validate_agent_private_key(encoded: &str) -> Result<()> {
    signing_key_from_pkcs8_b64(encoded)?;
    Ok(())
}

fn signing_key_from_pkcs8_b64(encoded: &str) -> Result<SigningKey> {
    let der = B64
        .decode(encoded.trim())
        .context("agent private key is not valid base64")?;
    SigningKey::from_pkcs8_der(&der).context("agent private key is not valid PKCS#8 Ed25519")
}

fn encode_ssh_ed25519_public_key(verifying_key: &VerifyingKey) -> String {
    let mut blob = Vec::with_capacity(4 + 11 + 4 + 32);
    append_ssh_string(&mut blob, b"ssh-ed25519");
    append_ssh_string(&mut blob, verifying_key.as_bytes());
    format!("ssh-ed25519 {}", B64.encode(blob))
}

fn append_ssh_string(buf: &mut Vec<u8>, value: &[u8]) {
    buf.extend_from_slice(&(value.len() as u32).to_be_bytes());
    buf.extend_from_slice(value);
}

/// Register a new agent runtime using a live ChatGPT access token.
pub async fn register_agent_runtime(
    access_token: &str,
    public_key_ssh: &str,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .timeout(REGISTER_TIMEOUT)
        .build()
        .context("build http client")?;
    let body = RegisterAgentRequest {
        abom: AgentBillOfMaterials {
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            agent_harness_id: "codex-cli".to_string(),
            running_location: format!("cli-{}", std::env::consts::OS),
        },
        agent_public_key: public_key_ssh.to_string(),
        capabilities: Vec::new(),
        ttl: None,
    };
    let url = format!("{AUTH_API_BASE}/v1/agent/register");
    let response = client
        .post(&url)
        .bearer_auth(access_token.trim())
        .json(&body)
        .send()
        .await
        .context("agent identity register request failed")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = text.chars().take(400).collect();
        bail!("agent/register failed ({status}): {snippet}");
    }
    let parsed: RegisterAgentResponse =
        serde_json::from_str(&text).context("agent/register response invalid")?;
    if parsed.agent_runtime_id.trim().is_empty() {
        bail!("agent/register omitted agent_runtime_id");
    }
    Ok(parsed.agent_runtime_id)
}

pub fn sign_task_registration(key: &AgentIdentityKey, timestamp: &str) -> Result<String> {
    let signing_key = signing_key_from_pkcs8_b64(&key.private_key_pkcs8_base64)?;
    let payload = format!("{}:{timestamp}", key.agent_runtime_id);
    Ok(B64.encode(signing_key.sign(payload.as_bytes()).to_bytes()))
}

pub async fn register_agent_task(key: &AgentIdentityKey) -> Result<String> {
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let signature = sign_task_registration(key, &timestamp)?;
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .timeout(TASK_TIMEOUT)
        .build()
        .context("build http client")?;
    let url = format!(
        "{AUTH_API_BASE}/v1/agent/{}/task/register",
        key.agent_runtime_id
    );
    let response = client
        .post(url)
        .json(&RegisterTaskRequest {
            timestamp,
            signature,
        })
        .send()
        .await
        .context("agent task registration request failed")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = text.chars().take(400).collect();
        bail!("task/register failed ({status}): {snippet}");
    }
    let parsed: RegisterTaskResponse =
        serde_json::from_str(&text).context("task/register response invalid")?;
    if let Some(task_id) = parsed
        .task_id
        .or(parsed.task_id_camel)
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(task_id);
    }
    let encrypted = parsed
        .encrypted_task_id
        .or(parsed.encrypted_task_id_camel)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow!("task/register omitted task id"))?;
    decrypt_task_id(key, &encrypted)
}

fn decrypt_task_id(key: &AgentIdentityKey, encrypted_b64: &str) -> Result<String> {
    let signing_key = signing_key_from_pkcs8_b64(&key.private_key_pkcs8_base64)?;
    let ciphertext = B64
        .decode(encrypted_b64.trim())
        .context("encrypted task id is not valid base64")?;
    // Derive Curve25519 secret from Ed25519 seed the same way Codex does.
    let digest = Sha512::digest(signing_key.to_bytes());
    let mut secret_bytes = [0u8; 32];
    secret_bytes.copy_from_slice(&digest[..32]);
    secret_bytes[0] &= 248;
    secret_bytes[31] &= 127;
    secret_bytes[31] |= 64;
    let secret = CurveSecretKey::from(secret_bytes);
    let plaintext = secret
        .unseal(ciphertext.as_slice())
        .map_err(|_| anyhow!("failed to decrypt encrypted task id"))?;
    let task_id = String::from_utf8(plaintext).context("decrypted task id is not utf-8")?;
    let task_id = task_id.trim().to_string();
    if task_id.is_empty() {
        bail!("decrypted task id is empty");
    }
    Ok(task_id)
}

pub fn authorization_header_for_agent_task(key: &AgentIdentityKey, task_id: &str) -> Result<String> {
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let signing_key = signing_key_from_pkcs8_b64(&key.private_key_pkcs8_base64)?;
    let payload = format!("{}:{task_id}:{timestamp}", key.agent_runtime_id);
    let signature = B64.encode(signing_key.sign(payload.as_bytes()).to_bytes());
    let envelope = AgentAssertionEnvelope {
        agent_runtime_id: key.agent_runtime_id.clone(),
        task_id: task_id.to_string(),
        timestamp,
        signature,
    };
    // Stable key order for assertion payload (BTreeMap).
    let map = BTreeMap::from([
        ("agent_runtime_id", envelope.agent_runtime_id.as_str()),
        ("signature", envelope.signature.as_str()),
        ("task_id", envelope.task_id.as_str()),
        ("timestamp", envelope.timestamp.as_str()),
    ]);
    let serialized = serde_json::to_vec(&map).context("serialize agent assertion")?;
    Ok(format!(
        "AgentAssertion {}",
        URL_SAFE_NO_PAD.encode(serialized)
    ))
}

/// Build a durable Agent Identity credential from a live ChatGPT access token.
pub async fn upgrade_access_token_to_agent_identity(
    access_token: &str,
    email: Option<String>,
    account_id: Option<String>,
    label: Option<String>,
) -> Result<CanonicalCredential> {
    let access_token = access_token.trim();
    if access_token.is_empty() {
        bail!("缺少 access_token");
    }
    if let Some(exp) = jwt_exp_unix(access_token) {
        let now = Utc::now().timestamp();
        if exp <= now {
            bail!("access_token 已过期，请重新从 chatgpt.com/api/auth/session 导出");
        }
    }
    let account_id = account_id
        .filter(|s| !s.trim().is_empty())
        .or_else(|| chatgpt_account_id_from_token(access_token))
        .ok_or_else(|| anyhow!("无法从 session 解析 chatgpt_account_id"))?;
    let keys = generate_agent_key_material()?;
    let runtime_id = register_agent_runtime(access_token, &keys.public_key_ssh).await?;
    // Optionally pre-register a task so the first proxy call is warm.
    let mut task_id = None;
    let key = AgentIdentityKey {
        agent_runtime_id: runtime_id.clone(),
        private_key_pkcs8_base64: keys.private_key_pkcs8_base64.clone(),
        task_id: None,
    };
    if let Ok(tid) = register_agent_task(&key).await {
        task_id = Some(tid);
    }
    let mut cred = CanonicalCredential {
        kind: CredentialKind::AgentIdentity,
        state: CredentialState::Refreshable,
        provider_id: "openai".into(),
        label,
        email,
        account_id: Some(account_id),
        expires_at: None,
        fingerprint: String::new(),
        refreshable: true,
        secret: SecretMaterial {
            agent_runtime_id: Some(runtime_id),
            agent_private_key: Some(keys.private_key_pkcs8_base64),
            task_id,
            ..SecretMaterial::default()
        },
    };
    // Fingerprint assigned later via assign_provider.
    let _ = &mut cred;
    Ok(cred)
}

/// If the secret already carries agent identity fields, build a key handle.
pub fn agent_key_from_secret(secret: &SecretMaterial) -> Option<AgentIdentityKey> {
    let runtime = secret.agent_runtime_id.as_deref()?.trim();
    let private = secret.agent_private_key.as_deref()?.trim();
    if runtime.is_empty() || private.is_empty() {
        return None;
    }
    Some(AgentIdentityKey {
        agent_runtime_id: runtime.to_string(),
        private_key_pkcs8_base64: private.to_string(),
        task_id: secret
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
    })
}

#[allow(dead_code)]
pub fn is_invalid_task_auth_error(status: u16, body: &str) -> bool {
    if status != 401 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    [
        "\"code\":\"invalid_task_id\"",
        "\"code\":\"task_not_found\"",
        "\"code\":\"task_expired\"",
        "invalid task_id",
        "invalid task id",
        "task not found",
        "task expired",
    ]
    .iter()
    .any(|m| compact.contains(&m.replace(' ', "")) || lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_sign_assertion() {
        let material = generate_agent_key_material().expect("key");
        validate_agent_private_key(&material.private_key_pkcs8_base64).expect("valid");
        assert!(material.public_key_ssh.starts_with("ssh-ed25519 "));
        let key = AgentIdentityKey {
            agent_runtime_id: "runtime-test".into(),
            private_key_pkcs8_base64: material.private_key_pkcs8_base64,
            task_id: Some("task-test".into()),
        };
        let header = authorization_header_for_agent_task(&key, "task-test").expect("assert");
        assert!(header.starts_with("AgentAssertion "));
    }

    #[test]
    fn task_registration_signature_stable() {
        let material = generate_agent_key_material().expect("key");
        let key = AgentIdentityKey {
            agent_runtime_id: "r1".into(),
            private_key_pkcs8_base64: material.private_key_pkcs8_base64,
            task_id: None,
        };
        let sig = sign_task_registration(&key, "2026-01-01T00:00:00Z").expect("sig");
        assert!(!sig.is_empty());
    }
}
