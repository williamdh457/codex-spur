//! Z Code custom-provider publisher.
//!
//! Z Code does not consume Codex `supported_reasoning_levels`, so its provider
//! config needs an explicit, narrowly-scoped capability projection.
//!
//! The managed provider is always named **SPUR**. Older installs may still use
//! the internal id `codex-spur-responses` or a UUID key that points at the local
//! relay; we reuse those when present and only create a new provider when none
//! match.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    catalog::RouteTarget,
    reasoning_map::{self, ReasoningProfileId},
};

/// Legacy fixed provider id used by early Spur → Z Code sync builds.
const LEGACY_SPUR_PROVIDER_ID: &str = "codex-spur-responses";
/// Display name shown in Z Code's provider list.
const SPUR_PROVIDER_NAME: &str = "SPUR";
const DEFAULT_RELAY_BASE_URL: &str = "http://127.0.0.1:17862/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodePublishOutcome {
    pub model_count: u32,
    pub removed_model_count: u32,
    pub config_path: String,
    pub backup_path: String,
    pub provider_id: String,
    pub provider_created: bool,
    pub warnings: Vec<String>,
}

/// Optional local relay coordinates used when Spur must create the SPUR provider.
#[derive(Debug, Clone, Default)]
pub struct ZcodeRelayEndpoint {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_path() -> PathBuf {
    std::env::var_os("CODEX_SPUR_ZCODE_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".zcode/v2/config.json"))
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Z Code config 路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("config.json"),
        timestamp()
    ));
    {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

fn profile(target: &RouteTarget) -> ReasoningProfileId {
    ReasoningProfileId::parse(&target.reasoning_profile_id)
        .unwrap_or_else(|| ReasoningProfileId::default_for_kind(&target.kind))
}

fn canonical_route_key(routes: &HashMap<String, RouteTarget>, target: &RouteTarget) -> String {
    let expected = crate::providers::model_part_for_slug(&target.upstream_model);
    let mut keys = routes
        .iter()
        .filter_map(|(key, candidate)| {
            (candidate.route_id == target.route_id
                && key.starts_with(&format!("{expected}."))
                && !key.starts_with("spur-route-"))
            .then_some(key.clone())
        })
        .collect::<Vec<_>>();
    keys.sort();
    keys.into_iter().next().unwrap_or_else(|| {
        format!(
            "{}.{}",
            expected,
            crate::providers::provider_part_for_slug("", &target.kind, &target.provider_id)
        )
    })
}

fn zcode_reasoning_spec(capability: &reasoning_map::ModelReasoningCapability) -> Value {
    let mut levels = Map::new();
    for level in &capability.levels {
        let patch = json!({
            "set": [{
                "path": ["reasoningEffort"],
                "value": level.as_str()
            }]
        });
        // Current Z Code installs may define the local provider as either
        // `openai` or `openai-compatible`. Publish both narrowly-scoped
        // patches; Z Code chooses the one for the provider's active kind.
        levels.insert(
            level.as_str().to_string(),
            json!({
                "openai": patch,
                "openai-compatible": {
                    "set": [{
                        "path": ["reasoningEffort"],
                        "value": level.as_str()
                    }]
                }
            }),
        );
    }
    json!({
        "defaultLevel": capability.default_level.map(|level| level.as_str()),
        "levels": levels
    })
}

fn model_entry(routes: &HashMap<String, RouteTarget>, target: &RouteTarget) -> (String, Value) {
    let capability = reasoning_map::model_reasoning_capability(
        &target.kind,
        &target.upstream_model,
        profile(target),
    );
    let context = if target.kind.eq_ignore_ascii_case("kimi") {
        crate::providers::kimi_context_window(&target.upstream_model)
    } else if target.kind.eq_ignore_ascii_case("xai") {
        500_000
    } else {
        128_000
    };
    let name = if target.kind.eq_ignore_ascii_case("kimi")
        && matches!(
            target.upstream_model.trim().to_ascii_lowercase().as_str(),
            "k3" | "kimi-k3"
        )
    {
        "Kimi code · K3".to_string()
    } else if target.kind.eq_ignore_ascii_case("kimi")
        && matches!(
            target.upstream_model.trim().to_ascii_lowercase().as_str(),
            "kimi-for-coding" | "kimi-k2.7-code"
        )
    {
        "Kimi code · K2.7 Coding".to_string()
    } else {
        target.upstream_model.clone()
    };
    let mut model = json!({
        "name": name,
        "limit": { "context": context },
        "modalities": { "input": ["text"], "output": ["text"] }
    });
    if capability.selectable {
        let variants = capability
            .levels
            .iter()
            .map(|level| level.as_str())
            .collect::<Vec<_>>();
        model["reasoning"] = json!({
            "enabled": true,
            "variants": variants,
            "defaultVariant": capability.default_level.map(|level| level.as_str())
        });
        model["zcode"] = json!({
            "reasoning": zcode_reasoning_spec(&capability)
        });
    } else if matches!(
        capability.upstream_mode,
        reasoning_map::ReasoningUpstreamMode::AlwaysOn
    ) {
        model["supportsReasoning"] = Value::Bool(true);
    }
    (canonical_route_key(routes, target), model)
}

fn normalize_base_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn looks_like_local_spur_relay(base_url: &str) -> bool {
    let base = normalize_base_url(base_url);
    base.contains("127.0.0.1:17862")
        || base.contains("localhost:17862")
        || base.contains("0.0.0.0:17862")
        || base.contains("[::1]:17862")
}

fn provider_name_of(provider: &Map<String, Value>) -> String {
    provider
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn provider_base_url_of(provider: &Map<String, Value>) -> String {
    provider
        .get("options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("baseURL").or_else(|| options.get("baseUrl")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Prefer an existing Spur-managed provider over creating a fresh one.
///
/// Match order:
/// 1. legacy fixed id `codex-spur-responses`
/// 2. display name exactly `SPUR` (case-insensitive)
/// 3. name/id containing spur, with local relay base URL
/// 4. any custom provider already pointed at the local Spur relay
fn find_spur_provider_id(providers: &Map<String, Value>) -> Option<String> {
    if providers.contains_key(LEGACY_SPUR_PROVIDER_ID) {
        return Some(LEGACY_SPUR_PROVIDER_ID.to_string());
    }

    let mut exact_name = None;
    let mut spurish_relay = None;
    let mut any_local_relay = None;

    for (id, value) in providers {
        let Some(provider) = value.as_object() else {
            continue;
        };
        let name = provider_name_of(provider);
        let name_l = name.to_ascii_lowercase();
        let base = provider_base_url_of(provider);
        let local_relay = looks_like_local_spur_relay(&base);
        let id_l = id.to_ascii_lowercase();
        let spurish = name_l == "spur"
            || name_l == "codex-spur-responses"
            || name_l.contains("codex spur")
            || name_l.contains("codex-spur")
            || id_l.contains("spur");

        if name.eq_ignore_ascii_case(SPUR_PROVIDER_NAME) && exact_name.is_none() {
            exact_name = Some(id.clone());
        }
        if spurish && local_relay && spurish_relay.is_none() {
            spurish_relay = Some(id.clone());
        }
        if local_relay && any_local_relay.is_none() {
            any_local_relay = Some(id.clone());
        }
    }

    exact_name.or(spurish_relay).or(any_local_relay)
}

fn default_spur_provider(endpoint: &ZcodeRelayEndpoint) -> Value {
    let base_url = endpoint
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_RELAY_BASE_URL)
        .to_string();
    let api_key = endpoint
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string();
    json!({
        "name": SPUR_PROVIDER_NAME,
        "kind": "openai",
        "source": "custom",
        "options": {
            "apiKey": api_key,
            "baseURL": base_url,
            "apiKeyRequired": true
        },
        "models": {}
    })
}

fn ensure_spur_provider_shape(provider: &mut Map<String, Value>, endpoint: &ZcodeRelayEndpoint) {
    provider.insert("name".into(), json!(SPUR_PROVIDER_NAME));
    if !provider.contains_key("kind") {
        provider.insert("kind".into(), json!("openai"));
    }
    if !provider.contains_key("source") {
        provider.insert("source".into(), json!("custom"));
    }

    let options = provider
        .entry("options".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(options) = options.as_object_mut() {
        if let Some(base_url) = endpoint
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            options.insert("baseURL".into(), json!(base_url));
        } else if !options.contains_key("baseURL") && !options.contains_key("baseUrl") {
            options.insert("baseURL".into(), json!(DEFAULT_RELAY_BASE_URL));
        }
        if let Some(api_key) = endpoint
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            // Only fill an empty key; never clobber a user-edited secret.
            let existing = options
                .get("apiKey")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if existing.is_empty() {
                options.insert("apiKey".into(), json!(api_key));
            }
        }
        options
            .entry("apiKeyRequired".to_string())
            .or_insert(json!(true));
    }

    if !provider
        .get("models")
        .and_then(Value::as_object)
        .is_some()
    {
        provider.insert("models".into(), Value::Object(Map::new()));
    }
}

/// Replace Spur's managed model set in Z Code's SPUR provider.
///
/// Z Code is an API relay client, so it must track only routes enabled for
/// Spur's relay rather than the independent Codex picker selection. If no
/// matching provider exists, create one named `SPUR`.
fn apply_at(
    path: &Path,
    routes: &HashMap<String, RouteTarget>,
    endpoint: &ZcodeRelayEndpoint,
) -> Result<ZcodePublishOutcome> {
    let raw = if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("读取 Z Code 配置失败：{}", path.display()))?
    } else {
        r#"{"provider":{}}"#.to_string()
    };
    let mut root: Value = serde_json::from_str(&raw).context("解析 Z Code config.json 失败")?;
    if !root.is_object() {
        bail!("Z Code config.json 根节点必须是对象");
    }
    let root_obj = root.as_object_mut().expect("object root");
    let provider_map = root_obj
        .entry("provider".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let providers = provider_map
        .as_object_mut()
        .ok_or_else(|| anyhow!("Z Code config.json 的 provider 不是对象"))?;

    let mut provider_created = false;
    let provider_id = if let Some(existing) = find_spur_provider_id(providers) {
        existing
    } else {
        provider_created = true;
        LEGACY_SPUR_PROVIDER_ID.to_string()
    };

    if provider_created {
        providers.insert(provider_id.clone(), default_spur_provider(endpoint));
    }

    let spur = providers
        .get_mut(&provider_id)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("Z Code SPUR provider 不是对象"))?;
    ensure_spur_provider_shape(spur, endpoint);

    let existing_model_count = spur
        .get("models")
        .and_then(Value::as_object)
        .map(|models| models.len())
        .unwrap_or(0);

    let mut unique = BTreeMap::new();
    for target in routes.values() {
        if target.relay_enabled {
            unique
                .entry(target.route_id.clone())
                .or_insert_with(|| target.clone());
        }
    }
    let mut projected_models = Map::new();
    for target in unique.values() {
        let (id, entry) = model_entry(routes, target);
        projected_models.insert(id, entry);
    }
    let removed_model_count = existing_model_count.saturating_sub(
        projected_models
            .keys()
            .filter(|id| {
                spur.get("models")
                    .and_then(Value::as_object)
                    .is_some_and(|models| models.contains_key(*id))
            })
            .count(),
    ) as u32;
    spur.insert("models".into(), Value::Object(projected_models));

    let backup = path.with_file_name(format!(
        "{}.bak-codex-spur-{}",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("config.json"),
        timestamp()
    ));
    if path.exists() {
        fs::copy(path, &backup)
            .with_context(|| format!("备份 Z Code 配置失败：{}", backup.display()))?;
    } else if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::write(&backup, raw.as_bytes())
            .with_context(|| format!("写入 Z Code 初始备份失败：{}", backup.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(&root)?;
    atomic_write(path, &bytes)?;
    let verify: Value = serde_json::from_slice(&fs::read(path)?).context("回读 Z Code 配置失败")?;
    if verify
        .get("provider")
        .and_then(|value| value.get(&provider_id))
        .is_none()
    {
        bail!("写入后校验失败：Z Code SPUR provider 丢失")
    }
    Ok(ZcodePublishOutcome {
        model_count: unique.len() as u32,
        removed_model_count,
        config_path: path.display().to_string(),
        backup_path: backup.display().to_string(),
        provider_id,
        provider_created,
        warnings: vec![
            "请在 Z Code 中重新加载配置或重启应用后查看 Thought Level。".into(),
            "Z Code 仅显示反代已开启的 Spur 模型；关闭反代后再次同步会移除对应条目。".into(),
            format!("Z Code 供应商名称固定为 {SPUR_PROVIDER_NAME}。"),
        ],
    })
}

pub fn apply(
    routes: &HashMap<String, RouteTarget>,
    endpoint: ZcodeRelayEndpoint,
) -> Result<ZcodePublishOutcome> {
    apply_at(&config_path(), routes, &endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kind: &str, model: &str) -> RouteTarget {
        RouteTarget {
            route_id: format!("{kind}/{model}"),
            provider_id: "kimi-code".into(),
            kind: kind.into(),
            upstream_model: model.into(),
            base_url: "http://127.0.0.1:17861/v1".into(),
            protocol: "Responses".into(),
            reasoning_profile_id: ReasoningProfileId::default_for_kind(kind).as_str().into(),
            codex_enabled: true,
            relay_enabled: false,
        }
    }

    #[test]
    fn projects_k3_and_grok_reasoning_for_zcode() {
        let mut routes = HashMap::new();
        let k3 = route("kimi", "k3");
        routes.insert("k3.kimi-code".into(), k3.clone());
        let (id, model) = model_entry(&routes, &k3);
        assert_eq!(id, "k3.kimi-code");
        assert_eq!(
            model["reasoning"]["variants"],
            json!(["low", "high", "max"])
        );
        assert_eq!(model["reasoning"]["defaultVariant"], "max");
        assert_eq!(
            model["zcode"]["reasoning"]["levels"]["max"]["openai"]["set"][0]["path"],
            json!(["reasoningEffort"])
        );
        assert_eq!(
            model["zcode"]["reasoning"]["levels"]["max"]["openai-compatible"]["set"][0]["value"],
            "max"
        );

        let grok = route("xai", "grok-4.5");
        routes.insert("grok-4.5.kimi-code".into(), grok.clone());
        let (_, model) = model_entry(&routes, &grok);
        assert_eq!(
            model["reasoning"]["variants"],
            json!(["low", "medium", "high"])
        );
        assert_eq!(model["reasoning"]["defaultVariant"], "high");
    }

    #[test]
    fn sync_replaces_stale_models_with_relay_selection_only() {
        let temp =
            std::env::temp_dir().join(format!("codex-spur-zcode-{}.json", uuid::Uuid::new_v4()));
        fs::write(
            &temp,
            r#"{
                "provider": {
                    "other": {"models": {"unrelated": {"name": "Keep"}}},
                    "codex-spur-responses": {
                        "name": "codex-spur-responses",
                        "kind": "openai",
                        "options": {"apiKey": "keep", "baseURL": "http://127.0.0.1:17862/v1"},
                        "models": {
                            "k3.kimi-code": {"name": "Old K3"},
                            "kimi-k3.kimi-code": {"name": "Wrong alias"},
                            "kimi-for-coding.kimi-code": {"name": "Stale K2.7"},
                            "kimi-for-coding-highspeed.kimi-code": {"name": "Stale highspeed"}
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let mut routes = HashMap::new();
        let mut k3 = route("kimi", "k3");
        k3.relay_enabled = true;
        routes.insert("k3.kimi-code".into(), k3);
        let k2 = route("kimi", "kimi-for-coding");
        routes.insert("kimi-for-coding.kimi-code".into(), k2);

        let outcome = apply_at(&temp, &routes, &ZcodeRelayEndpoint::default()).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        let models = &root["provider"][LEGACY_SPUR_PROVIDER_ID]["models"];
        assert_eq!(outcome.model_count, 1);
        assert_eq!(outcome.removed_model_count, 3);
        assert_eq!(outcome.provider_id, LEGACY_SPUR_PROVIDER_ID);
        assert!(!outcome.provider_created);
        assert_eq!(models.as_object().unwrap().len(), 1);
        assert!(models.get("k3.kimi-code").is_some());
        assert!(models.get("kimi-k3.kimi-code").is_none());
        assert_eq!(
            models["k3.kimi-code"]["zcode"]["reasoning"]["defaultLevel"],
            "max"
        );
        assert_eq!(
            root["provider"]["other"]["models"]["unrelated"]["name"],
            "Keep"
        );
        assert_eq!(
            root["provider"][LEGACY_SPUR_PROVIDER_ID]["options"]["apiKey"],
            "keep"
        );
        assert_eq!(
            root["provider"][LEGACY_SPUR_PROVIDER_ID]["name"],
            SPUR_PROVIDER_NAME
        );
    }

    #[test]
    fn sync_allows_empty_relay_selection() {
        let temp =
            std::env::temp_dir().join(format!("codex-spur-zcode-{}.json", uuid::Uuid::new_v4()));
        fs::write(
            &temp,
            r#"{"provider":{"codex-spur-responses":{"models":{"old":{"name":"Old"}}}}}"#,
        )
        .unwrap();

        let outcome = apply_at(&temp, &HashMap::new(), &ZcodeRelayEndpoint::default()).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        assert_eq!(outcome.model_count, 0);
        assert_eq!(outcome.removed_model_count, 1);
        assert_eq!(
            root["provider"][LEGACY_SPUR_PROVIDER_ID]["models"],
            Value::Object(Map::new())
        );
        assert_eq!(
            root["provider"][LEGACY_SPUR_PROVIDER_ID]["name"],
            SPUR_PROVIDER_NAME
        );
    }

    #[test]
    fn sync_creates_spur_provider_when_missing() {
        let temp =
            std::env::temp_dir().join(format!("codex-spur-zcode-{}.json", uuid::Uuid::new_v4()));
        fs::write(&temp, r#"{"provider":{"other":{"models":{}}}}"#).unwrap();

        let mut routes = HashMap::new();
        let mut grok = route("xai", "grok-4.5");
        grok.relay_enabled = true;
        grok.provider_id = "0868".into();
        routes.insert("grok-4.5.0868".into(), grok);

        let endpoint = ZcodeRelayEndpoint {
            base_url: Some("http://127.0.0.1:17862/v1".into()),
            api_key: Some("sk-spur-test".into()),
        };
        let outcome = apply_at(&temp, &routes, &endpoint).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        assert!(outcome.provider_created);
        assert_eq!(outcome.provider_id, LEGACY_SPUR_PROVIDER_ID);
        assert_eq!(outcome.model_count, 1);
        let spur = &root["provider"][LEGACY_SPUR_PROVIDER_ID];
        assert_eq!(spur["name"], SPUR_PROVIDER_NAME);
        assert_eq!(spur["options"]["apiKey"], "sk-spur-test");
        assert_eq!(spur["options"]["baseURL"], "http://127.0.0.1:17862/v1");
        assert_eq!(
            spur["models"]["grok-4.5.0868"]["reasoning"]["variants"],
            json!(["low", "medium", "high"])
        );
        assert_eq!(
            spur["models"]["grok-4.5.0868"]["reasoning"]["defaultVariant"],
            "high"
        );
    }

    #[test]
    fn sync_reuses_uuid_provider_pointing_at_local_relay() {
        let temp =
            std::env::temp_dir().join(format!("codex-spur-zcode-{}.json", uuid::Uuid::new_v4()));
        fs::write(
            &temp,
            r#"{
                "provider": {
                    "3f396ed6-015e-475f-8b31-ad7af3bb1379": {
                        "name": "codex-spur-responses",
                        "kind": "openai",
                        "options": {
                            "apiKey": "sk-keep",
                            "baseURL": "http://127.0.0.1:17862/v1"
                        },
                        "models": {
                            "grok-4.5.0868": {
                                "limit": {"context": 500000},
                                "zcode": {"modified": true}
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let mut routes = HashMap::new();
        let mut grok = route("xai", "grok-4.5");
        grok.relay_enabled = true;
        grok.provider_id = "0868".into();
        routes.insert("grok-4.5.0868".into(), grok);

        let outcome = apply_at(&temp, &routes, &ZcodeRelayEndpoint::default()).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        assert!(!outcome.provider_created);
        assert_eq!(
            outcome.provider_id,
            "3f396ed6-015e-475f-8b31-ad7af3bb1379"
        );
        let spur = &root["provider"]["3f396ed6-015e-475f-8b31-ad7af3bb1379"];
        assert_eq!(spur["name"], SPUR_PROVIDER_NAME);
        assert_eq!(spur["options"]["apiKey"], "sk-keep");
        assert_eq!(
            spur["models"]["grok-4.5.0868"]["reasoning"]["variants"],
            json!(["low", "medium", "high"])
        );
    }
}
