use std::time::Duration;

use anyhow::{anyhow, Context};
use reqwest::header::{HeaderMap, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    CatalogModel, ReasoningEffort, ReasoningEffortPreset, ReasoningMapping, ReasoningProfile,
    TruncationPolicy,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredProviderModel {
    pub id: String,
    pub display_name: String,
    pub owned_by: Option<String>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCatalogPayload {
    pub model: CatalogModel,
    pub reasoning_profile: ReasoningProfile,
}

pub fn normalize_base_url(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("Base URL 不能为空"));
    }
    let parsed = url::Url::parse(trimmed).context("Base URL 不是有效 URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(anyhow!("Base URL 必须使用 http 或 https"));
    }
    Ok(trimmed.to_string())
}

/// Template metadata for a provider kind (not a single instance).
pub fn kind_meta(kind: &str) -> Option<(&'static str, &'static str, &'static str, Option<&'static str>)> {
    match kind {
        "openai" => Some((
            "OpenAI",
            "Official",
            "Responses",
            Some("https://chatgpt.com/backend-api/codex"),
        )),
        "kimi" => Some((
            "Kimi Code",
            "中国 / Global",
            "Chat Completions",
            Some("https://api.kimi.com/coding/v1"),
        )),
        "opencode-go" => Some((
            "OpenCode Go",
            "Global",
            "Chat Completions",
            Some(crate::opencode_go::DEFAULT_BASE_URL),
        )),
        "deepseek" => Some((
            "DeepSeek",
            "Global",
            "Chat Completions",
            Some("https://api.deepseek.com/v1"),
        )),
        "minimax" => Some((
            "MiniMax",
            "中国 / Global",
            "Responses preferred",
            Some("https://api.minimaxi.com/v1"),
        )),
        "xai" => Some((
            "Grok",
            "Global",
            "Responses",
            Some("https://api.x.ai/v1"),
        )),
        "custom" => Some(("自定义供应商", "Custom", "OpenAI-compatible", None)),
        _ => None,
    }
}

pub fn default_base_url_for_kind(kind: &str) -> Option<String> {
    kind_meta(kind)
        .and_then(|(_, _, _, url)| url.map(str::to_string))
}

#[allow(dead_code)]
pub fn kind_display_name(kind: &str) -> &'static str {
    kind_meta(kind).map(|(name, _, _, _)| name).unwrap_or("Custom")
}

/// Codex official catalog requires client_version; keep aligned with gated model min versions.
pub const CODEX_CLIENT_VERSION: &str = "0.144.1";
pub const CODEX_ORIGINATOR: &str = "codex_cli_rs";

/// xAI API-key traffic (and kind_meta default for form API setup).
pub const XAI_API_BASE: &str = "https://api.x.ai/v1";
/// Grok OAuth / SuperGrok subscription traffic must use the CLI chat proxy.
/// Tokens from device-code login are rejected or 4xx on `api.x.ai` without CLI identity.
pub const XAI_CLI_SUBSCRIPTION_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
/// Stable client version string for CLI-proxy identity headers.
pub const XAI_CLI_CLIENT_VERSION: &str = "0.2.93";
pub const XAI_CLI_USER_AGENT: &str = "Codex-Spur-Grok/0.1";

/// True when `base_url` points at an official xAI host that is **not** a user custom gateway.
/// Empty, `api.x.ai`, and the CLI subscription host all count as "not customized".
pub fn is_xai_official_host(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return true;
    }
    let Ok(parsed) = url::Url::parse(trimmed) else {
        let lower = trimmed.to_ascii_lowercase();
        return lower.contains("api.x.ai") || lower.contains("cli-chat-proxy.grok.com");
    };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    host == "api.x.ai" || host == "cli-chat-proxy.grok.com"
}

/// Resolve the upstream base for an xAI instance.
///
/// - **Subscription / OAuth (`official` or `subscription_oauth`)**: CLI proxy when the
///   stored base is empty or an official host; keep true custom hosts.
/// - **API key / other**: `api.x.ai` default, or the stored base when set.
pub fn resolve_xai_upstream_base(entry_category: Option<&str>, stored_base: Option<&str>) -> String {
    let stored = stored_base.map(str::trim).filter(|s| !s.is_empty());
    let is_subscription = matches!(
        entry_category.map(str::to_ascii_lowercase).as_deref(),
        Some("official") | Some("subscription") | Some("oauth")
    );
    if is_subscription {
        match stored {
            Some(url) if !is_xai_official_host(url) => url.trim_end_matches('/').to_string(),
            _ => XAI_CLI_SUBSCRIPTION_BASE.to_string(),
        }
    } else {
        match stored {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => XAI_API_BASE.to_string(),
        }
    }
}

/// True when outbound requests to this base need Grok CLI identity headers.
pub fn xai_base_needs_cli_headers(base_url: &str) -> bool {
    base_url
        .to_ascii_lowercase()
        .contains("cli-chat-proxy.grok.com")
}

/// Strict provider-config JSON (user chose this import path — no credential sniffing).
#[derive(Debug, Clone)]
pub struct ProviderConfigImport {
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Vec<DiscoveredProviderModel>,
}

fn looks_like_account_json(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(looks_like_account_json),
        Value::Object(map) => {
            if map.contains_key("accounts") {
                return true;
            }
            let auth = map
                .get("authJson")
                .or_else(|| map.get("auth_json"))
                .or_else(|| map.get("credentials"))
                .and_then(Value::as_object)
                .unwrap_or(map);
            let tokens = auth
                .get("tokens")
                .and_then(Value::as_object)
                .or_else(|| map.get("tokens").and_then(Value::as_object));
            let has = |key: &str| {
                auth.get(key)
                    .or_else(|| map.get(key))
                    .or_else(|| tokens.and_then(|t| t.get(key)))
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            };
            has("access_token") || has("refresh_token") || has("session_token")
        }
        _ => false,
    }
}

fn string_field_from_map(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = map.get(*key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            return Some(value.to_string());
        }
    }
    None
}

fn extract_base_url_from_object(map: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(url) = string_field_from_map(
        map,
        &[
            "base_url",
            "baseUrl",
            "api_base",
            "apiBase",
            "endpoint",
            "openai_base_url",
            "OPENAI_BASE_URL",
            "ANTHROPIC_BASE_URL",
        ],
    ) {
        return Some(url);
    }
    if let Some(env) = map
        .get("settingsConfig")
        .and_then(|v| v.get("env"))
        .and_then(Value::as_object)
        .or_else(|| map.get("env").and_then(Value::as_object))
    {
        if let Some(url) = string_field_from_map(
            env,
            &[
                "OPENAI_BASE_URL",
                "ANTHROPIC_BASE_URL",
                "base_url",
                "baseUrl",
            ],
        ) {
            return Some(url);
        }
    }
    if let Some(nested) = map
        .get("provider")
        .and_then(Value::as_object)
        .or_else(|| map.get("config").and_then(Value::as_object))
    {
        return extract_base_url_from_object(nested);
    }
    None
}

fn extract_api_key_from_object(map: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(key) = string_field_from_map(map, &["api_key", "apiKey", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]) {
        return Some(key);
    }
    if let Some(env) = map
        .get("settingsConfig")
        .and_then(|v| v.get("env"))
        .and_then(Value::as_object)
        .or_else(|| map.get("env").and_then(Value::as_object))
    {
        if let Some(key) = string_field_from_map(env, &["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "api_key", "apiKey"]) {
            return Some(key);
        }
    }
    if let Some(nested) = map
        .get("provider")
        .and_then(Value::as_object)
        .or_else(|| map.get("config").and_then(Value::as_object))
    {
        return extract_api_key_from_object(nested);
    }
    None
}

fn parse_models_array(items: &[Value]) -> Vec<DiscoveredProviderModel> {
    items
        .iter()
        .filter_map(|item| {
            if let Some(id) = item.as_str() {
                let id = id.trim();
                if id.is_empty() {
                    return None;
                }
                return Some(DiscoveredProviderModel {
                    id: id.to_string(),
                    display_name: id.to_string(),
                    owned_by: None,
                    created_at: None,
                });
            }
            let id = item
                .get("id")
                .or_else(|| item.get("slug"))
                .or_else(|| item.get("name"))
                .or_else(|| item.get("model"))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            if id.is_empty() {
                return None;
            }
            Some(DiscoveredProviderModel {
                display_name: item
                    .get("display_name")
                    .or_else(|| item.get("displayName"))
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                owned_by: item
                    .get("owned_by")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                created_at: None,
                id,
            })
        })
        .collect()
}

fn parse_config_object(
    map: &serde_json::Map<String, Value>,
    fallback_base_url: Option<&str>,
) -> anyhow::Result<ProviderConfigImport> {
    let base_url_raw = extract_base_url_from_object(map)
        .or_else(|| fallback_base_url.map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned))
        .ok_or_else(|| {
            anyhow!("供应商配置 JSON 缺少 base_url（也接受 baseUrl / api_base / endpoint / OPENAI_BASE_URL）")
        })?;
    let base_url = normalize_base_url(&base_url_raw)?;
    let api_key = extract_api_key_from_object(map);
    let models = map
        .get("models")
        .and_then(Value::as_array)
        .map(|items| parse_models_array(items))
        .unwrap_or_default();
    Ok(ProviderConfigImport {
        base_url,
        api_key,
        models,
    })
}

#[allow(dead_code)]
pub fn parse_provider_config_json(input: &str) -> anyhow::Result<ProviderConfigImport> {
    parse_provider_config_json_with_fallback(input, None)
}

pub fn parse_provider_config_json_with_fallback(
    input: &str,
    fallback_base_url: Option<&str>,
) -> anyhow::Result<ProviderConfigImport> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| anyhow!("不是有效的供应商配置 JSON（需要 JSON 对象或数组）"))?;
    if looks_like_account_json(&value) {
        return Err(anyhow!(
            "这是账号/auth JSON，不是供应商配置。请改用「OpenAI · 导入账号 JSON（账号池）」或「OpenAI · 官方订阅（浏览器登录）」。"
        ));
    }
    match &value {
        Value::Object(map) => {
            if let Some(providers) = map.get("providers").and_then(Value::as_array) {
                for item in providers {
                    if let Some(obj) = item.as_object() {
                        if let Ok(parsed) = parse_config_object(obj, fallback_base_url) {
                            return Ok(parsed);
                        }
                    }
                }
            }
            parse_config_object(map, fallback_base_url)
        }
        Value::Array(items) => {
            for item in items {
                if let Some(obj) = item.as_object() {
                    if let Ok(parsed) = parse_config_object(obj, fallback_base_url) {
                        return Ok(parsed);
                    }
                }
            }
            Err(anyhow!("供应商配置 JSON 数组里没有可用的 base_url 对象"))
        }
        _ => Err(anyhow!("不是有效的供应商配置 JSON（根节点必须是对象或数组）")),
    }
}

pub async fn discover_official_models(
    access_token: &str,
    account_id: &str,
) -> anyhow::Result<Vec<DiscoveredProviderModel>> {
    discover_official_models_with_authorization(
        &format!("Bearer {access_token}"),
        account_id,
    )
    .await
}

/// Discover ChatGPT Codex models with a full Authorization header value
/// (`Bearer …` or `AgentAssertion …`).
pub async fn discover_official_models_with_authorization(
    authorization: &str,
    account_id: &str,
) -> anyhow::Result<Vec<DiscoveredProviderModel>> {
    let client = reqwest::Client::builder()
        .user_agent(format!("codex_cli_rs/{}", CODEX_CLIENT_VERSION))
        .build()?;
    let models_url = format!(
        "https://chatgpt.com/backend-api/codex/models?client_version={}",
        CODEX_CLIENT_VERSION
    );
    let response = client
        .get(models_url)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header("ChatGPT-Account-Id", account_id)
        .header("chatgpt-account-id", account_id)
        .header("originator", CODEX_ORIGINATOR)
        .header("version", CODEX_CLIENT_VERSION)
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = response.status();
    let raw_body = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&raw_body)
        .map_err(|_| anyhow!("官方 Codex 模型列表返回不是 JSON（{}）", status))?;
    if !status.is_success() {
        let detail = body
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| body.get("detail").and_then(Value::as_str))
            .unwrap_or_else(|| raw_body.lines().next().unwrap_or("上游拒绝请求"));
        let detail = detail.chars().take(280).collect::<String>();
        return Err(anyhow!("官方模型列表请求失败（{}）：{}", status, detail));
    }
    let mut models = Vec::new();
    if let Some(values) = body
        .get("data")
        .or_else(|| body.get("models"))
        .or_else(|| body.get("items"))
        .and_then(Value::as_array)
    {
        models.extend(parse_models_array(values));
    }
    if let Some(map) = body.get("models").and_then(Value::as_object) {
        for (key, entry) in map {
            if entry.is_object() || entry.is_string() {
                let mut one = parse_models_array(std::slice::from_ref(entry));
                if one.is_empty() {
                    let id = key.trim();
                    if !id.is_empty() {
                        models.push(DiscoveredProviderModel {
                            id: id.to_string(),
                            display_name: id.to_string(),
                            owned_by: Some("OpenAI subscription".into()),
                            created_at: None,
                        });
                    }
                } else {
                    models.append(&mut one);
                }
            }
        }
    }
    for model in &mut models {
        if model.owned_by.is_none() {
            model.owned_by = Some("OpenAI subscription".into());
        }
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    if models.is_empty() {
        return Err(anyhow!("官方模型列表为空"));
    }
    Ok(models)
}

fn kimi_coding_models() -> Vec<DiscoveredProviderModel> {
    [
        ("kimi-for-coding", "Kimi For Coding"),
        ("kimi-k2.7-code", "Kimi K2.7 Code"),
        ("kimi-k2.5", "Kimi K2.5"),
        ("kimi-k2-0905-preview", "Kimi K2 0905 Preview"),
        ("kimi-k2-turbo-preview", "Kimi K2 Turbo Preview"),
    ]
    .into_iter()
    .map(|(id, display_name)| DiscoveredProviderModel {
        id: id.into(),
        display_name: display_name.into(),
        owned_by: Some("Moonshot AI".into()),
        created_at: None,
    })
    .collect()
}

/// Curated Grok subscription models when `/v1/models` is empty or incomplete.
pub fn xai_subscription_models() -> Vec<DiscoveredProviderModel> {
    [
        ("grok-build-0.1", "Grok Build 0.1"),
        ("grok-4.5", "Grok 4.5"),
        ("grok-4.3", "Grok 4.3"),
        ("grok-4.20-0309-reasoning", "Grok 4.20 (Reasoning)"),
    ]
    .into_iter()
    .map(|(id, display_name)| DiscoveredProviderModel {
        id: id.into(),
        display_name: display_name.into(),
        owned_by: Some("xAI".into()),
        created_at: None,
    })
    .collect()
}

/// Prefer live discovery; fall back to the subscription catalog when the list
/// is empty, unauthorized-for-list, or missing coding models.
pub async fn discover_xai_models(
    base_url: &str,
    access_token: Option<&str>,
) -> anyhow::Result<Vec<DiscoveredProviderModel>> {
    match discover_models("xai", base_url, access_token).await {
        Ok(models) if !models.is_empty() => {
            let has_coding = models.iter().any(|m| {
                let id = m.id.to_ascii_lowercase();
                id.contains("grok-build") || id.contains("grok-4")
            });
            if has_coding {
                Ok(models)
            } else {
                // Keep API models and append curated coding models not already present.
                let mut merged = models;
                let existing: std::collections::HashSet<String> =
                    merged.iter().map(|m| m.id.clone()).collect();
                for extra in xai_subscription_models() {
                    if !existing.contains(&extra.id) {
                        merged.push(extra);
                    }
                }
                Ok(merged)
            }
        }
        _ => Ok(xai_subscription_models()),
    }
}

pub async fn discover_models(
    provider_id: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<DiscoveredProviderModel>> {
    let base_url = normalize_base_url(base_url)?;
    let endpoint = if base_url.ends_with("/v1") {
        format!("{base_url}/models")
    } else {
        format!("{base_url}/v1/models")
    };
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut headers = HeaderMap::new();
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        let value = format!("Bearer {}", api_key.trim());
        headers.insert(
            AUTHORIZATION,
            value.parse().context("API key 无法生成 Authorization")?,
        );
    }
    let response = client.get(&endpoint).headers(headers).send().await?;
    let status = response.status();
    let raw_body = response.text().await?;
    let body = serde_json::from_str::<Value>(&raw_body).ok();
    if !status.is_success() {
        if provider_id == "kimi" && status == reqwest::StatusCode::NOT_FOUND {
            return Ok(kimi_coding_models());
        }
        let message = body
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| raw_body.lines().next().unwrap_or("上游模型列表请求失败"));
        let detail = message.chars().take(240).collect::<String>();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "模型列表请求失败（401）：{}。上游需要 Bearer。请到「API 配置」填写 API Key，或先在「导入 JSON」导入账号后再拉取。",
                detail
            ));
        }
        return Err(anyhow!(
            "模型列表请求失败（{}）：{}",
            status,
            detail
        ));
    }
    let body = body.ok_or_else(|| anyhow!("模型列表返回不是 JSON（{}）。Kimi for Coding 请使用 https://api.kimi.com/coding/v1，而不是 www.kimi.com/code/v1。", status))?;
    let values = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("模型列表缺少 data/models 数组"))?;
    let mut models = values
        .iter()
        .filter_map(|item| {
            let id = item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            if id.is_empty() {
                return None;
            }
            Some(DiscoveredProviderModel {
                display_name: item
                    .get("display_name")
                    .or_else(|| item.get("displayName"))
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                owned_by: item
                    .get("owned_by")
                    .or_else(|| item.get("ownedBy"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                created_at: item
                    .get("created")
                    .or_else(|| item.get("created_at"))
                    .and_then(Value::as_i64),
                id,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    if models.is_empty() {
        return Err(anyhow!("上游返回了空模型列表"));
    }
    tracing::info!(
        provider = provider_id,
        count = models.len(),
        "discovered provider models"
    );
    Ok(models)
}

pub fn reasoning_profile(kind: &str, model_id: &str) -> ReasoningProfile {
    let (title, upstream_levels): (&str, [&str; 8]) = match kind {
        "deepseek" => (
            "DeepSeek 两档推理映射",
            [
                "disabled", "disabled", "disabled", "enabled", "enabled", "enabled", "enabled",
                "enabled",
            ],
        ),
        "kimi" => (
            "Kimi 多档推理映射",
            [
                "off", "low", "low", "medium", "high", "high", "high", "high",
            ],
        ),
        "minimax" => (
            "MiniMax 单档推理映射",
            [
                "default", "default", "default", "default", "default", "default", "default",
                "default",
            ],
        ),
        "xai" => (
            // Grok 4.x exposes low/medium/high; higher Codex rungs clamp to high.
            // none/minimal still map to low rather than inventing a disable switch.
            "xAI Grok 三档推理映射",
            ["low", "low", "low", "medium", "high", "high", "high", "high"],
        ),
        _ => (
            "OpenAI 兼容推理映射",
            [
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ],
        ),
    };
    let mappings = ReasoningEffort::ALL
        .into_iter()
        .zip(upstream_levels)
        .map(|(codex_effort, upstream_effort)| ReasoningMapping {
            codex_effort,
            upstream_effort: upstream_effort.to_string(),
            explanation: format!(
                "Codex {} → {} {}",
                codex_effort.as_str(),
                kind,
                upstream_effort
            ),
        })
        .collect();
    ReasoningProfile {
        title: format!("{title} · {model_id}"),
        mappings,
    }
}

/// Codex GUI product copy for reasoning chips (aligned with CC Switch heal template).
/// Internal proxy mapping stays in `reasoning_profile`; catalog descriptions must not expose
/// upstream patch strings like `max → max`.
pub fn codex_reasoning_level_description(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "Disable Thinking",
        ReasoningEffort::Minimal => "Fastest responses with minimal reasoning",
        ReasoningEffort::Low => "Fast responses with lighter reasoning",
        ReasoningEffort::Medium => "Balances speed and reasoning depth for everyday tasks",
        ReasoningEffort::High => "Greater reasoning depth for complex problems",
        ReasoningEffort::Xhigh => "Extra high reasoning depth for complex problems",
        ReasoningEffort::Max => "Maximum reasoning depth for the hardest problems",
        ReasoningEffort::Ultra => "Maximum reasoning with automatic task delegation",
    }
}

pub const CODEX_AGENT_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

/// Ensure a catalog row matches working third-party catalogs (CC/Nice Switch).
///
/// Stale SQLite `catalog_json` often carries OpenAI-only tool flags. ChatGPT's custom
/// model picker has been observed to show an empty list when third-party rows advertise
/// shell/apply_patch/web_search like native GPT rows. Heal to the Nice Switch Kimi shape.
pub fn normalize_catalog_model_for_codex(model: &mut CatalogModel, priority: i32) {
    normalize_catalog_model_for_codex_with_kind(model, priority, None);
}

pub fn normalize_catalog_model_for_codex_with_kind(
    model: &mut CatalogModel,
    priority: i32,
    kind: Option<&str>,
) {
    model.shell_type = "shell_command".into();
    model.priority = priority;
    model.visibility = "list".into();
    model.supported_in_api = true;
    if model.base_instructions.trim().is_empty() {
        model.base_instructions = CODEX_AGENT_BASE_INSTRUCTIONS.into();
    }
    model.truncation_policy = TruncationPolicy {
        mode: "bytes".into(),
        limit: 10_000,
    };
    if model.effective_context_window_percent <= 0 || model.effective_context_window_percent < 95 {
        model.effective_context_window_percent = 95;
    }
    // Product ladder (CC Switch heal): no minimal; max/ultra keep official English copy.
    model.supported_reasoning_levels = ReasoningEffort::ALL
        .into_iter()
        .map(|effort| ReasoningEffortPreset {
            effort,
            description: codex_reasoning_level_description(effort).into(),
        })
        .collect();
    if model.default_reasoning_level.is_none()
        || matches!(model.default_reasoning_level, Some(ReasoningEffort::Minimal))
    {
        model.default_reasoning_level = Some(ReasoningEffort::Medium);
    }
    // High-end efforts (max/ultra) need reasoning summaries enabled for full Codex chrome.
    model.supports_reasoning_summaries = true;

    let kind = kind.unwrap_or("").to_ascii_lowercase();
    let looks_openai = kind == "openai"
        || model.display_name.to_ascii_lowercase().contains("gpt")
        || model.slug.contains("gpt")
        || is_desktop_native_model_slug(&model.slug);

    // Critical Desktop invariant (verified against working CC Switch catalogs and the
    // ChatGPT Desktop model picker): custom `model_catalog_json` rows must stay lean.
    // Advertising shell/apply_patch/web_search on ANY custom-provider row — even GPT-named
    // ones — has been observed to empty the bottom-right model list. CC Switch keeps
    // experimental_supported_tools: [] for gpt-5.6-terra/sol/luna catalog rows.
    model.apply_patch_tool_type = None;
    model.web_search_tool_type = None;
    model.supports_parallel_tool_calls = false;
    model.experimental_supported_tools = Vec::new();
    model.include_skills_usage_instructions = false;
    model.supports_reasoning_summary_parameter = false;
    model.use_responses_lite = false;
    model.additional_speed_tiers.clear();
    model.service_tiers.clear();
    model.default_service_tier = None;
    model.availability_nux = None;
    model.upgrade = None;
    model.model_messages = None;
    model.default_verbosity = None;
    model.comp_hash = None;
    model.auto_review_model_override = None;
    model.tool_mode = None;
    model.multi_agent_version = None;

    if looks_openai {
        if model.context_window.unwrap_or(0) < 200_000 {
            model.context_window = Some(272_000);
            model.max_context_window = Some(272_000);
            model.auto_compact_token_limit = Some(244_800);
        }
        model.input_modalities = vec!["text".into(), "image".into()];
    } else {
        model.input_modalities = vec!["text".into(), "image".into()];
        if kind == "kimi" || model.display_name.to_ascii_lowercase().contains("kimi") {
            model.context_window = Some(262_144);
            model.max_context_window = Some(262_144);
            model.auto_compact_token_limit = Some(235_929);
        } else if model.context_window.unwrap_or(0) == 0 {
            model.context_window = Some(128_000);
            model.max_context_window = Some(128_000);
            model.auto_compact_token_limit = Some(115_200);
        }
    }
}

/// Stable opaque catalog/proxy slug (Nice Switch style). No `/`, no provider UUID leak.
pub fn opaque_route_slug(provider_id: &str, upstream_model: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"codex-spur-route-v1\0");
    hasher.update(provider_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(upstream_model.as_bytes());
    let digest = hasher.finalize();
    // 12 bytes → 24 hex chars, enough collision resistance for local route tables.
    format!("spur-route-{}", hex::encode(&digest[..12]))
}

/// ChatGPT Desktop's simple power picker only matches these official model slugs
/// (`gpt-5.6-terra` / `gpt-5.6-sol` / ultra). CC Switch catalogs use them verbatim.
/// Returning them as the published catalog slug makes the bottom-right picker show
/// real names instead of “自定义 / Custom”.
pub fn desktop_native_model_slug(upstream_model: &str) -> Option<&'static str> {
    let normalized = upstream_model.trim().to_ascii_lowercase();
    // Strip common provider prefixes: "openai/gpt-5.6-terra", "models/gpt-5.6-sol".
    let tail = normalized
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(&normalized);
    match tail {
        "gpt-5.6-terra" | "gpt-5-6-terra" => Some("gpt-5.6-terra"),
        "gpt-5.6-sol" | "gpt-5-6-sol" => Some("gpt-5.6-sol"),
        "gpt-5.6-luna" | "gpt-5-6-luna" => Some("gpt-5.6-luna"),
        _ => None,
    }
}

pub fn is_desktop_native_model_slug(slug: &str) -> bool {
    desktop_native_model_slug(slug).is_some()
        || matches!(
            slug.trim().to_ascii_lowercase().as_str(),
            "gpt-5.6-terra" | "gpt-5.6-sol" | "gpt-5.6-luna"
        )
}

/// Prefer Desktop-native public slug when unique; otherwise fall back to opaque.
///
/// Multi-instance OpenAI providers may both expose `gpt-5.6-terra` — only the first
/// enabled route may claim the public slug; later ones stay opaque so catalog ids
/// remain unique while the proxy still dual-keys both forms.
pub fn catalog_publish_slug(
    provider_id: &str,
    upstream_model: &str,
    claimed_public_slugs: &mut std::collections::HashSet<String>,
) -> String {
    if let Some(public) = desktop_native_model_slug(upstream_model) {
        if claimed_public_slugs.insert(public.to_string()) {
            return public.to_string();
        }
    }
    opaque_route_slug(provider_id, upstream_model)
}

/// Legacy DB / catalog slug used before opaque routes.
pub fn legacy_route_slug(provider_id: &str, upstream_model: &str) -> String {
    format!("{provider_id}/{}", slugify(upstream_model))
}

pub fn catalog_model(
    provider_id: &str,
    kind: &str,
    provider_label: &str,
    model: &DiscoveredProviderModel,
) -> CatalogModel {
    let supported_reasoning_levels = ReasoningEffort::ALL
        .into_iter()
        .map(|effort| ReasoningEffortPreset {
            effort,
            description: codex_reasoning_level_description(effort).into(),
        })
        .collect();
    // GPT-class models used with ChatGPT backend expect larger windows; others keep 128k default.
    let is_openai_family = kind == "openai"
        || model.id.contains("gpt-5")
        || model.id.contains("gpt-4");
    let (context_window, max_context_window) = if is_openai_family {
        (Some(272_000), Some(272_000))
    } else if kind == "kimi" {
        // Match working Nice Switch Kimi surface windows.
        (Some(262_144), Some(262_144))
    } else if kind == "xai" {
        // Grok Build ~256k; Grok 4.5 ~500k — use the larger window so catalog
        // does not under-advertise; proxy does not enforce this client-side.
        if model.id.contains("build") {
            (Some(256_000), Some(256_000))
        } else {
            (Some(500_000), Some(500_000))
        }
    } else {
        (Some(128_000), Some(128_000))
    };
    let auto_compact = context_window.map(|window| (window as f64 * 0.9) as i64);
    // Match CC Switch: catalog rows never advertise shell/apply_patch tools. Prefer the
    // Desktop-native public slug for terra/sol/luna so the power picker is non-empty.
    let slug = desktop_native_model_slug(&model.id)
        .map(str::to_string)
        .unwrap_or_else(|| opaque_route_slug(provider_id, &model.id));
    let mut catalog = CatalogModel {
        slug,
        display_name: format!("{} · {}", provider_label, model.display_name),
        description: Some(format!("{} · {}", provider_label, model.display_name)),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels,
        shell_type: "shell_command".into(),
        visibility: "list".into(),
        supported_in_api: true,
        priority: 1000,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: CODEX_AGENT_BASE_INSTRUCTIONS.into(),
        model_messages: None,
        include_skills_usage_instructions: false,
        supports_reasoning_summaries: true,
        supports_reasoning_summary_parameter: false,
        default_reasoning_summary: Value::String("auto".into()),
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: None,
        truncation_policy: TruncationPolicy {
            mode: "bytes".into(),
            limit: 10_000,
        },
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window,
        max_context_window,
        auto_compact_token_limit: auto_compact,
        comp_hash: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: if is_openai_family || kind == "kimi" || kind == "xai" {
            vec!["text".into(), "image".into()]
        } else {
            vec!["text".into()]
        },
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    };
    // Keep reasoning_profile mapping independent; catalog already has product copy.
    let _ = reasoning_profile(kind, &model.id);
    // Always pass kind so Kimi/DeepSeek rows are healed lean (no shell/apply_patch
    // ads that make ChatGPT Desktop show an empty model picker).
    normalize_catalog_model_for_codex_with_kind(&mut catalog, 1000, Some(kind));
    catalog
}

pub fn route_catalog_json(
    provider_id: &str,
    kind: &str,
    provider_label: &str,
    model: &DiscoveredProviderModel,
) -> anyhow::Result<String> {
    let payload = RouteCatalogPayload {
        model: catalog_model(provider_id, kind, provider_label, model),
        reasoning_profile: reasoning_profile(kind, &model.id),
    };
    Ok(serde_json::to_string(&payload)?)
}

#[allow(dead_code)]
pub fn provider_name(kind_or_id: &str) -> &'static str {
    kind_display_name(kind_or_id)
}

pub fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn secret_material_json(
    secret: &crate::credentials::SecretMaterial,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "access_token": secret.access_token,
        "refresh_token": secret.refresh_token,
        "id_token": secret.id_token,
        "session_token": secret.session_token,
        "api_key": secret.api_key,
        "agent_runtime_id": secret.agent_runtime_id,
        "agent_private_key": secret.agent_private_key,
        "task_id": secret.task_id,
    }))?)
}

pub fn credential_secret_json(
    credential: &crate::credentials::CanonicalCredential,
) -> anyhow::Result<Vec<u8>> {
    secret_material_json(&credential.secret)
}

pub async fn test_credential(
    provider_id: &str,
    base_url: &str,
    model_id: &str,
    secret: &crate::credentials::SecretMaterial,
) -> anyhow::Result<()> {
    let base_url = normalize_base_url(base_url)?;
    let endpoint_base = if base_url.ends_with("/v1") {
        base_url
    } else {
        format!("{base_url}/v1")
    };
    let token = secret
        .api_key
        .as_deref()
        .or(secret.access_token.as_deref())
        .or(secret.session_token.as_deref())
        .ok_or_else(|| anyhow!("账号没有可用的访问凭据"))?;
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .build()?;
    let mut request = if provider_id == "deepseek" {
        client
            .post(format!("{endpoint_base}/chat/completions"))
            .json(&serde_json::json!({
                "model": model_id,
                "messages": [{"role": "user", "content": "Hi"}],
                "stream": false,
            }))
    } else {
        client
            .post(format!("{endpoint_base}/responses"))
            .json(&serde_json::json!({
                "model": model_id,
                "input": "Hi",
                "stream": false,
            }))
    };
    request = request.bearer_auth(token);
    let response = request.send().await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let snippet = body
        .chars()
        .filter(|ch| !ch.is_control())
        .take(240)
        .collect::<String>();
    Err(anyhow!("账号测试失败（{}）：{}", status, snippet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codex_level_has_a_mapping() {
        for provider in ["kimi", "deepseek", "minimax", "opencode-go", "custom", "xai"] {
            assert_eq!(reasoning_profile(provider, "model").mappings.len(), 8);
        }
    }

    #[test]
    fn opencode_go_kind_meta_uses_chat_completions_endpoint() {
        let meta = kind_meta("opencode-go").expect("OpenCode Go kind");
        assert_eq!(meta.0, "OpenCode Go");
        assert_eq!(meta.2, "Chat Completions");
        assert_eq!(meta.3, Some(crate::opencode_go::DEFAULT_BASE_URL));
        assert_eq!(
            default_base_url_for_kind("opencode-go").as_deref(),
            Some(crate::opencode_go::DEFAULT_BASE_URL)
        );
    }

    #[test]
    fn xai_kind_meta_and_subscription_catalog() {
        let meta = kind_meta("xai").expect("xai kind");
        assert_eq!(meta.0, "Grok");
        assert_eq!(meta.3, Some(XAI_API_BASE));
        let models = xai_subscription_models();
        assert!(models.iter().any(|m| m.id == "grok-build-0.1"));
        assert!(models.iter().any(|m| m.id == "grok-4.5"));
        let profile = reasoning_profile("xai", "grok-4.5");
        assert_eq!(profile.mappings.len(), 8);
        assert_eq!(profile.mappings[4].upstream_effort, "high");
    }

    #[test]
    fn xai_subscription_base_resolves_to_cli_proxy() {
        assert_eq!(
            resolve_xai_upstream_base(Some("official"), None),
            XAI_CLI_SUBSCRIPTION_BASE
        );
        assert_eq!(
            resolve_xai_upstream_base(Some("official"), Some(XAI_API_BASE)),
            XAI_CLI_SUBSCRIPTION_BASE
        );
        assert_eq!(
            resolve_xai_upstream_base(Some("official"), Some("https://api.x.ai/v1/")),
            XAI_CLI_SUBSCRIPTION_BASE
        );
        // Legacy rows that already stored CLI host stay on CLI.
        assert_eq!(
            resolve_xai_upstream_base(Some("official"), Some(XAI_CLI_SUBSCRIPTION_BASE)),
            XAI_CLI_SUBSCRIPTION_BASE
        );
        // Explicit custom host is preserved.
        assert_eq!(
            resolve_xai_upstream_base(Some("official"), Some("https://my-relay.example/v1")),
            "https://my-relay.example/v1"
        );
    }

    #[test]
    fn xai_api_key_base_stays_on_api_x_ai() {
        assert_eq!(
            resolve_xai_upstream_base(Some("api"), None),
            XAI_API_BASE
        );
        assert_eq!(
            resolve_xai_upstream_base(Some("api"), Some(XAI_API_BASE)),
            XAI_API_BASE
        );
        assert_eq!(
            resolve_xai_upstream_base(None, Some("https://custom.x.example/v1")),
            "https://custom.x.example/v1"
        );
    }

    #[test]
    fn xai_cli_headers_only_for_subscription_host() {
        assert!(xai_base_needs_cli_headers(XAI_CLI_SUBSCRIPTION_BASE));
        assert!(!xai_base_needs_cli_headers(XAI_API_BASE));
        assert!(!xai_base_needs_cli_headers("https://example.com/v1"));
    }

    #[test]
    fn catalog_json_uses_snake_case_for_codex() {
        let model = catalog_model(
            "kimi-instance",
            "kimi",
            "Kimi tian",
            &DiscoveredProviderModel {
                id: "k3".into(),
                display_name: "K3".into(),
                owned_by: Some("Moonshot AI".into()),
                created_at: None,
            },
        );
        let json = serde_json::to_value(&model).expect("serialize");
        assert!(json.get("display_name").is_some(), "expected snake_case display_name");
        assert!(json.get("displayName").is_none(), "must not emit camelCase displayName");
        assert!(json.get("supported_reasoning_levels").is_some());
        assert!(json.get("supportedReasoningLevels").is_none());
        assert!(json.get("default_reasoning_level").is_some());
        let levels = json
            .get("supported_reasoning_levels")
            .and_then(|v| v.as_array())
            .expect("levels");
        assert_eq!(levels.len(), 8);
        let efforts: Vec<&str> = levels
            .iter()
            .filter_map(|level| level.get("effort").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            efforts,
            vec![
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"
            ]
        );
        let max_desc = levels
            .iter()
            .find(|level| level.get("effort").and_then(|v| v.as_str()) == Some("max"))
            .and_then(|level| level.get("description").and_then(|v| v.as_str()))
            .unwrap_or_default();
        assert_eq!(
            max_desc,
            "Maximum reasoning depth for the hardest problems"
        );
        assert_eq!(json.get("shell_type").and_then(|v| v.as_str()), Some("shell_command"));
        assert!(
            json.get("base_instructions")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .contains("You are Codex"),
            "base_instructions should match Codex agent blurb"
        );
        let slug = json.get("slug").and_then(|v| v.as_str()).unwrap_or_default();
        assert!(slug.starts_with("spur-route-"), "expected opaque slug, got {slug}");
        assert!(!slug.contains('/'), "opaque slug must not contain '/'");
        // ChatGPT's GUI expects the complete ModelInfo shape even for empty values.
        assert_eq!(json.get("additional_speed_tiers"), Some(&serde_json::json!([])));
        assert_eq!(json.get("service_tiers"), Some(&serde_json::json!([])));
        assert_eq!(json.get("upgrade"), Some(&serde_json::Value::Null));
        assert_eq!(json.get("availability_nux"), Some(&serde_json::Value::Null));
        assert!(json.get("model_messages").is_none());
        // Codex requires bool fields present even when false.
        assert_eq!(json.get("support_verbosity"), Some(&serde_json::json!(false)));
    }

    #[test]
    fn opaque_route_slug_is_stable_and_slash_free() {
        let a = opaque_route_slug("prov-a", "k3");
        let b = opaque_route_slug("prov-a", "k3");
        let c = opaque_route_slug("prov-b", "k3");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("spur-route-"));
        assert!(!a.contains('/'));
    }

    #[test]
    fn catalog_publish_slug_prefers_desktop_native_once() {
        let mut claimed = std::collections::HashSet::new();
        let first = catalog_publish_slug("p1", "gpt-5.6-terra", &mut claimed);
        let second = catalog_publish_slug("p2", "gpt-5.6-terra", &mut claimed);
        let kimi = catalog_publish_slug("p3", "k3", &mut claimed);
        assert_eq!(first, "gpt-5.6-terra");
        assert!(second.starts_with("spur-route-"), "second claim stays opaque: {second}");
        assert!(kimi.starts_with("spur-route-"));
        assert_eq!(desktop_native_model_slug("openai/gpt-5.6-sol"), Some("gpt-5.6-sol"));
    }

    #[test]
    fn openai_catalog_rows_do_not_advertise_tools() {
        let model = catalog_model(
            "openai-instance",
            "openai",
            "OpenAI 2",
            &DiscoveredProviderModel {
                id: "gpt-5.6-terra".into(),
                display_name: "GPT-5.6-Terra".into(),
                owned_by: None,
                created_at: None,
            },
        );
        assert_eq!(model.slug, "gpt-5.6-terra");
        assert_eq!(model.display_name, "OpenAI 2 · GPT-5.6-Terra");
        assert!(model.experimental_supported_tools.is_empty());
        assert!(model.apply_patch_tool_type.is_none());
        assert!(!model.supports_parallel_tool_calls);
    }

    #[test]
    fn catalog_model_accepts_legacy_camel_case() {
        let raw = r#"{
            "slug": "x/k3",
            "displayName": "Kimi · K3",
            "description": "legacy",
            "defaultReasoningLevel": "medium",
            "supportedReasoningLevels": [{"effort":"high","description":"high"}],
            "shellType": "default",
            "visibility": "list",
            "supportedInApi": true,
            "priority": 0
        }"#;
        let model: CatalogModel = serde_json::from_str(raw).expect("legacy camelCase");
        assert_eq!(model.display_name, "Kimi · K3");
        assert_eq!(model.supported_reasoning_levels.len(), 1);
    }

    #[test]
    fn normalizes_provider_base_urls() {
        assert_eq!(
            normalize_base_url("https://example.com/v1/").unwrap(),
            "https://example.com/v1"
        );
        assert!(normalize_base_url("file:///tmp/models").is_err());
    }

    #[test]
    fn parses_strict_provider_config_json() {
        let parsed = parse_provider_config_json(
            r#"{"base_url":"https://api.example.com/v1","api_key":"sk-test","models":["m1",{"id":"m2","display_name":"Model 2"}]}"#,
        )
        .expect("config");
        assert_eq!(parsed.base_url, "https://api.example.com/v1");
        assert_eq!(parsed.api_key.as_deref(), Some("sk-test"));
        assert_eq!(parsed.models.len(), 2);
        assert_eq!(parsed.models[1].display_name, "Model 2");
    }

    #[test]
    fn rejects_provider_config_without_base_url() {
        let err = parse_provider_config_json(r#"{"api_key":"sk-test"}"#).unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[test]
    fn rejects_account_json_on_config_path() {
        let err = parse_provider_config_json(
            r#"{"access_token":"tok","refresh_token":"ref","account_id":"acc"}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("账号"));
    }

    #[test]
    fn secret_material_json_keeps_agent_and_usage_tokens() {
        let secret = crate::credentials::SecretMaterial {
            access_token: Some("access".into()),
            refresh_token: Some("refresh".into()),
            agent_runtime_id: Some("runtime".into()),
            agent_private_key: Some("pk".into()),
            task_id: Some("task".into()),
            ..Default::default()
        };
        let bytes = secret_material_json(&secret).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(value["access_token"], "access");
        assert_eq!(value["refresh_token"], "refresh");
        assert_eq!(value["agent_runtime_id"], "runtime");
        assert_eq!(value["agent_private_key"], "pk");
        assert_eq!(value["task_id"], "task");
    }

    #[test]
    fn parses_config_with_fallback_base_url() {
        let parsed = parse_provider_config_json_with_fallback(
            r#"{"api_key":"sk-test","models":["kimi-for-coding"]}"#,
            Some("https://api.kimi.com/coding/v1"),
        )
        .expect("config");
        assert_eq!(parsed.base_url, "https://api.kimi.com/coding/v1");
        assert_eq!(parsed.models[0].id, "kimi-for-coding");
    }

    #[test]
    fn parses_nested_openai_base_url_env() {
        let parsed = parse_provider_config_json(
            r#"{"settingsConfig":{"env":{"OPENAI_BASE_URL":"https://api.example.com/v1","OPENAI_API_KEY":"sk-x"}},"models":["m1"]}"#,
        )
        .expect("config");
        assert_eq!(parsed.base_url, "https://api.example.com/v1");
        assert_eq!(parsed.api_key.as_deref(), Some("sk-x"));
    }
}
