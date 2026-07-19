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

pub async fn discover_official_models(
    access_token: &str,
    account_id: &str,
) -> anyhow::Result<Vec<DiscoveredProviderModel>> {
    let client = reqwest::Client::builder()
        .user_agent("Codex-Spur/0.1")
        .build()?;
    let response = client
        .get("https://chatgpt.com/backend-api/codex/models")
        .bearer_auth(access_token)
        .header("ChatGPT-Account-Id", account_id)
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .context("官方 Codex 模型列表返回不是 JSON")?;
    if !status.is_success() {
        return Err(anyhow!(
            "官方模型列表请求失败（{}）：{}",
            status,
            body.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游拒绝请求")
        ));
    }
    let values = body
        .get("models")
        .or_else(|| body.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("官方模型列表缺少 models/data 数组"))?;
    let mut models = values
        .iter()
        .filter_map(|item| {
            let id = item
                .get("slug")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)?
                .to_string();
            Some(DiscoveredProviderModel {
                display_name: item
                    .get("display_name")
                    .or_else(|| item.get("displayName"))
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string(),
                owned_by: Some("OpenAI subscription".into()),
                created_at: None,
                id,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    if models.is_empty() {
        return Err(anyhow!("官方模型列表为空"));
    }
    Ok(models)
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
        .build()?;
    let mut headers = HeaderMap::new();
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        let value = format!("Bearer {}", api_key.trim());
        headers.insert(
            AUTHORIZATION,
            value.parse().context("API key 无法生成 Authorization")?,
        );
    }
    let response = client.get(endpoint).headers(headers).send().await?;
    let status = response.status();
    let body: Value = response.json().await.context("模型列表返回不是 JSON")?;
    if !status.is_success() {
        let message = body
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("上游模型列表请求失败");
        return Err(anyhow!("{} ({})", message, status));
    }
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

pub fn reasoning_profile(provider_id: &str, model_id: &str) -> ReasoningProfile {
    let (title, upstream_levels): (&str, [&str; 8]) = match provider_id {
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
                provider_id,
                upstream_effort
            ),
        })
        .collect();
    ReasoningProfile {
        title: format!("{title} · {model_id}"),
        mappings,
    }
}

pub fn catalog_model(provider_id: &str, model: &DiscoveredProviderModel) -> CatalogModel {
    let profile = reasoning_profile(provider_id, &model.id);
    let supported_reasoning_levels = profile
        .mappings
        .iter()
        .map(|mapping| ReasoningEffortPreset {
            effort: mapping.codex_effort,
            description: format!(
                "{} → {}",
                mapping.codex_effort.as_str(),
                mapping.upstream_effort
            ),
        })
        .collect();
    CatalogModel {
        slug: format!("{provider_id}/{}", slugify(&model.id)),
        display_name: format!("{} · {}", provider_name(provider_id), model.display_name),
        description: Some(format!("{} model routed by Codex Spur", model.id)),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels,
        shell_type: "default".into(),
        visibility: "list".into(),
        supported_in_api: true,
        priority: 0,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: String::new(),
        model_messages: None,
        include_skills_usage_instructions: false,
        supports_reasoning_summary_parameter: false,
        default_reasoning_summary: Value::String("auto".into()),
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: Some("freeform".into()),
        web_search_tool_type: "text".into(),
        truncation_policy: TruncationPolicy {
            mode: "tokens".into(),
            limit: 0,
        },
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(128_000),
        max_context_window: Some(128_000),
        auto_compact_token_limit: Some(115_200),
        comp_hash: None,
        effective_context_window_percent: 90,
        experimental_supported_tools: vec!["shell".into(), "apply_patch".into()],
        input_modalities: vec!["text".into()],
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

pub fn route_catalog_json(
    provider_id: &str,
    model: &DiscoveredProviderModel,
) -> anyhow::Result<String> {
    let payload = RouteCatalogPayload {
        model: catalog_model(provider_id, model),
        reasoning_profile: reasoning_profile(provider_id, &model.id),
    };
    Ok(serde_json::to_string(&payload)?)
}

pub fn provider_name(provider_id: &str) -> &'static str {
    match provider_id {
        "kimi" => "Kimi",
        "deepseek" => "DeepSeek",
        "minimax" => "MiniMax",
        "openai" => "OpenAI",
        _ => "Custom",
    }
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

pub fn credential_secret_json(
    credential: &crate::credentials::CanonicalCredential,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::json!({
        "access_token": credential.secret.access_token,
        "refresh_token": credential.secret.refresh_token,
        "id_token": credential.secret.id_token,
        "session_token": credential.secret.session_token,
        "api_key": credential.secret.api_key,
    }))?)
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
        for provider in ["kimi", "deepseek", "minimax", "custom"] {
            assert_eq!(reasoning_profile(provider, "model").mappings.len(), 8);
        }
    }

    #[test]
    fn normalizes_provider_base_urls() {
        assert_eq!(
            normalize_base_url("https://example.com/v1/").unwrap(),
            "https://example.com/v1"
        );
        assert!(normalize_base_url("file:///tmp/models").is_err());
    }
}
