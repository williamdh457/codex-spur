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

fn zcode_model_id(routes: &HashMap<String, RouteTarget>, target: &RouteTarget) -> String {
    let canonical = canonical_route_key(routes, target);
    if target.kind.eq_ignore_ascii_case("kimi")
        && matches!(
            target.upstream_model.trim().to_ascii_lowercase().as_str(),
            "k3" | "kimi-k3"
        )
    {
        return canonical.replacen("k3.", "kimi-k3.", 1);
    }
    canonical
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
    } else if matches!(
        capability.upstream_mode,
        reasoning_map::ReasoningUpstreamMode::AlwaysOn
    ) {
        model["supportsReasoning"] = Value::Bool(true);
    }
    (zcode_model_id(routes, target), model)
}

/// Merge Spur's currently published routes into Z Code's existing provider.
/// This intentionally fails if the provider was never configured: silently
/// creating a new arbitrary provider could overwrite a user's intended setup.
pub fn apply(routes: &HashMap<String, RouteTarget>) -> Result<ZcodePublishOutcome> {
    let path = config_path();
    let raw = fs::read_to_string(&path)
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
    let models = spur
        .entry("models")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow!("Z Code Spur provider.models 不是对象"))?;

    let mut unique = BTreeMap::new();
    for target in routes.values() {
        if target.codex_enabled || target.relay_enabled {
            unique
                .entry(target.route_id.clone())
                .or_insert_with(|| target.clone());
        }
    }
    if unique.is_empty() {
        bail!("没有已发布的 Spur 路由可同步到 Z Code")
    }
    for target in unique.values() {
        let (id, entry) = model_entry(routes, target);
        let existing = models
            .entry(id)
            .or_insert_with(|| Value::Object(Map::new()));
        let existing_object = existing
            .as_object_mut()
            .ok_or_else(|| anyhow!("Z Code 模型配置不是对象"))?;
        let projected = entry.as_object().expect("model entry object");
        for (key, value) in projected {
            existing_object.insert(key.clone(), value.clone());
        }
    }

    let backup = path.with_file_name(format!(
        "{}.bak-codex-spur-{}",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("config.json"),
        timestamp()
    ));
    fs::copy(&path, &backup)
        .with_context(|| format!("备份 Z Code 配置失败：{}", backup.display()))?;
    let bytes = serde_json::to_vec_pretty(&root)?;
    atomic_write(&path, &bytes)?;
    let verify: Value =
        serde_json::from_slice(&fs::read(&path)?).context("回读 Z Code 配置失败")?;
    if verify
        .get("provider")
        .and_then(|value| value.get(SPUR_PROVIDER_ID))
        .is_none()
    {
        bail!("写入后校验失败：Z Code Spur provider 丢失")
    }
    Ok(ZcodePublishOutcome {
        model_count: unique.len() as u32,
        config_path: path.display().to_string(),
        backup_path: backup.display().to_string(),
        warnings: vec![
            "请在 Z Code 中重新加载配置或重启应用后查看 Thought Level。".into(),
            "Kimi K3 以 kimi-k3.* 别名发布；旧 k3.* 会继续由 Spur 本地代理路由。".into(),
        ],
    })
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
        assert_eq!(id, "kimi-k3.kimi-code");
        assert_eq!(
            model["reasoning"]["variants"],
            json!(["low", "high", "max"])
        );
        assert_eq!(model["reasoning"]["defaultVariant"], "max");

        let grok = route("xai", "grok-4.5");
        routes.insert("grok-4.5.kimi-code".into(), grok.clone());
        let (_, model) = model_entry(&routes, &grok);
        assert_eq!(
            model["reasoning"]["variants"],
            json!(["low", "medium", "high"])
        );
        assert_eq!(model["reasoning"]["defaultVariant"], "high");
    }
}
