//! Z Code custom-provider publisher.
//!
//! Z Code does not consume Codex `supported_reasoning_levels`, so its provider
//! config needs an explicit, narrowly-scoped capability projection.

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

const SPUR_PROVIDER_ID: &str = "codex-spur-responses";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZcodePublishOutcome {
    pub model_count: u32,
    pub removed_model_count: u32,
    pub config_path: String,
    pub backup_path: String,
    pub warnings: Vec<String>,
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
        ) {
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

/// Replace Spur's managed model set in Z Code's existing provider.
///
/// Z Code is an API relay client, so it must track only routes enabled for
/// Spur's relay rather than the independent Codex picker selection. This
/// intentionally fails if the provider was never configured: silently
/// creating a new arbitrary provider could overwrite a user's intended setup.
fn apply_at(path: &Path, routes: &HashMap<String, RouteTarget>) -> Result<ZcodePublishOutcome> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取 Z Code 配置失败：{}", path.display()))?;
    let mut root: Value = serde_json::from_str(&raw).context("解析 Z Code config.json 失败")?;
    let provider = root
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("Z Code config.json 缺少 provider 对象"))?;
    let spur = provider
        .get_mut(SPUR_PROVIDER_ID)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("Z Code 未配置 {SPUR_PROVIDER_ID}；请先在 Z Code 添加 Codex Spur Responses provider"))?;
    let existing_model_count = spur
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Z Code Spur provider.models 不是对象"))?
        .len();

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
    fs::copy(path, &backup)
        .with_context(|| format!("备份 Z Code 配置失败：{}", backup.display()))?;
    let bytes = serde_json::to_vec_pretty(&root)?;
    atomic_write(path, &bytes)?;
    let verify: Value = serde_json::from_slice(&fs::read(path)?).context("回读 Z Code 配置失败")?;
    if verify
        .get("provider")
        .and_then(|value| value.get(SPUR_PROVIDER_ID))
        .is_none()
    {
        bail!("写入后校验失败：Z Code Spur provider 丢失")
    }
    Ok(ZcodePublishOutcome {
        model_count: unique.len() as u32,
        removed_model_count,
        config_path: path.display().to_string(),
        backup_path: backup.display().to_string(),
        warnings: vec![
            "请在 Z Code 中重新加载配置或重启应用后查看 Thought Level。".into(),
            "Z Code 仅显示反代已开启的 Spur 模型；关闭反代后再次同步会移除对应条目。".into(),
        ],
    })
}

pub fn apply(routes: &HashMap<String, RouteTarget>) -> Result<ZcodePublishOutcome> {
    apply_at(&config_path(), routes)
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
                        "kind": "openai",
                        "options": {"apiKey": "keep", "baseURL": "http://127.0.0.1:17861/v1"},
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

        let outcome = apply_at(&temp, &routes).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        let models = &root["provider"][SPUR_PROVIDER_ID]["models"];
        assert_eq!(outcome.model_count, 1);
        assert_eq!(outcome.removed_model_count, 3);
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
            root["provider"][SPUR_PROVIDER_ID]["options"]["apiKey"],
            "keep"
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

        let outcome = apply_at(&temp, &HashMap::new()).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&temp).unwrap()).unwrap();
        assert_eq!(outcome.model_count, 0);
        assert_eq!(outcome.removed_model_count, 1);
        assert_eq!(
            root["provider"][SPUR_PROVIDER_ID]["models"],
            Value::Object(Map::new())
        );
    }
}
