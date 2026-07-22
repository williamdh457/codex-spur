//! Experimental Kimi Desktop (GUI) publisher — config/cache only.
//!
//! Does **not** modify `/Applications/Kimi.app`. Writes under
//! `~/Library/Application Support/kimi-desktop/**` with backups + atomic replace.
//! Online cloud sync may wipe custom models; see `docs/kimi-app-phase0-probe.md`.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::catalog::RouteTarget;

const SUPPORTED_KIMI_VERSION: &str = "3.1.3";
const SPUR_PROVIDER_ID: &str = "spur-gateway";
const SPUR_PROVIDER_TYPE: &str = "kimi";
const MARKER_FILE: &str = ".codex-spur-kimi-publish.json";
const ACTIVE_FILE: &str = ".codex-spur-kimi-active";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiTargetStatus {
    pub installed: bool,
    pub app_version: Option<String>,
    pub version_supported: bool,
    pub user_dir: String,
    pub cache_path: String,
    pub config_path: String,
    pub runtime_toml_path: String,
    pub control_url: Option<String>,
    pub control_ready: bool,
    pub last_publish_at: Option<String>,
    pub last_model_count: Option<u32>,
    /// Explicit on/off from last 启用/关闭发布 action (persisted).
    pub publish_active: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiPublishPreview {
    pub experimental: bool,
    pub kimi_version: Option<String>,
    pub gateway_base_url: String,
    pub model_count: u32,
    pub model_labels: Vec<String>,
    pub cache_path: String,
    pub config_path: String,
    pub runtime_toml_path: String,
    pub cache_preview: String,
    pub config_diff_summary: String,
    pub toml_diff_summary: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiPublishOutcome {
    pub experimental: bool,
    pub model_count: u32,
    pub model_labels: Vec<String>,
    pub backup_dir: String,
    pub cache_path: String,
    pub config_path: String,
    pub runtime_toml_path: String,
    pub control_updated: bool,
    pub restart_recommended: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishMarker {
    published_at: String,
    model_count: u32,
    model_aliases: Vec<String>,
    gateway_base_url: String,
    kimi_version: Option<String>,
    backup_dir: String,
}

#[derive(Debug, Clone)]
struct PlannedModel {
    alias: String,
    display_name: String,
    description: String,
    route_slug: String,
    upstream_model: String,
    provider_kind: String,
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn kimi_user_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("CODEX_SPUR_KIMI_USER_DIR") {
        return PathBuf::from(override_dir);
    }
    home_dir().join("Library/Application Support/kimi-desktop")
}

pub fn kimi_app_path() -> PathBuf {
    PathBuf::from("/Applications/Kimi.app")
}

fn cache_path(user_dir: &Path) -> PathBuf {
    user_dir.join("kimi-agent/kimi-work-models-cache.json")
}

fn config_path(user_dir: &Path) -> PathBuf {
    user_dir.join("daimon-share/daimon/config.json")
}

fn runtime_toml_path(user_dir: &Path) -> PathBuf {
    user_dir.join("daimon-share/daimon/runtime/kimi-code/config.toml")
}

fn runner_state_path(user_dir: &Path) -> PathBuf {
    user_dir.join("daimon-share/daimon/agents/main/runner.state.json")
}

fn marker_path(user_dir: &Path) -> PathBuf {
    user_dir.join("daimon-share/daimon").join(MARKER_FILE)
}

fn spur_backup_root(user_dir: &Path) -> PathBuf {
    user_dir.join("daimon-share/daimon/codex-spur-backups")
}

fn read_kimi_app_version() -> Option<String> {
    let plist = kimi_app_path().join("Contents/Info.plist");
    let text = fs::read_to_string(plist).ok()?;
    // Prefer CFBundleShortVersionString
    let key = "<key>CFBundleShortVersionString</key>";
    let idx = text.find(key)?;
    let after = &text[idx + key.len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].trim().to_string())
}

fn load_marker(user_dir: &Path) -> Option<PublishMarker> {
    let raw = fs::read_to_string(marker_path(user_dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn control_endpoint(user_dir: &Path) -> Option<(String, String)> {
    let raw = fs::read_to_string(runner_state_path(user_dir)).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let url = v
        .pointer("/control/endpoint/url")
        .and_then(Value::as_str)?
        .to_string();
    let token = v
        .pointer("/control/endpoint/auth/token")
        .and_then(Value::as_str)?
        .to_string();
    if url.is_empty() || token.is_empty() {
        return None;
    }
    Some((url, token))
}

pub fn inspect_status() -> KimiTargetStatus {
    let user_dir = kimi_user_dir();
    let installed = kimi_app_path().exists() || user_dir.exists();
    let app_version = read_kimi_app_version();
    let version_supported = app_version
        .as_deref()
        .map(|v| v == SUPPORTED_KIMI_VERSION || v.starts_with("3.1."))
        .unwrap_or(false);
    let mut warnings = Vec::new();
    if !kimi_app_path().exists() {
        warnings.push("未检测到 /Applications/Kimi.app".into());
    }
    if !user_dir.exists() {
        warnings.push("未检测到 Kimi 用户目录（请先启动一次 Kimi Desktop）".into());
    }
    if let Some(ver) = &app_version {
        if !version_supported {
            warnings.push(format!(
                "Kimi 版本 {ver} 未经 Phase 0 指纹验证（基线 {SUPPORTED_KIMI_VERSION}）"
            ));
        }
    }
    warnings.push(
        "实验性：右下角列表默认听云端；要显示 Spur 模型请拦 DescribeKimiWorkConfig 后冷启动（见 docs/kimi-app-selective-block.md）。".into(),
    );
    let marker = load_marker(&user_dir);
    let control = control_endpoint(&user_dir);
    let cache_spur = count_spur_models_in_cache(&user_dir);
    if let Some(n) = cache_spur {
        if n == 0 {
            warnings.push(
                "当前 kimi-work-models-cache 中无 spur-* 条目（可能被在线 sync 覆盖）。请先「发布到 Kimi」。"
                    .into(),
            );
        } else {
            warnings.push(format!(
                "cache 中现有 {n} 个 spur-* 模型条目（断网/路径拦截后冷启动才可能出现在右下角）。"
            ));
        }
    }
    let publish_active = is_publish_active(&user_dir);
    if publish_active {
        warnings.push("发布已启用（若 Kimi 右下角仍无 Spur 模型，请完全退出并重开 Kimi）。".into());
    }
    KimiTargetStatus {
        installed,
        app_version,
        version_supported,
        user_dir: user_dir.display().to_string(),
        cache_path: cache_path(&user_dir).display().to_string(),
        config_path: config_path(&user_dir).display().to_string(),
        runtime_toml_path: runtime_toml_path(&user_dir).display().to_string(),
        control_url: control.as_ref().map(|(u, _)| u.clone()),
        control_ready: control.is_some(),
        last_publish_at: marker.as_ref().map(|m| m.published_at.clone()),
        last_model_count: marker.as_ref().map(|m| m.model_count),
        publish_active,
        warnings,
    }
}

fn active_flag_path(user_dir: &Path) -> PathBuf {
    user_dir
        .join("daimon-share/daimon")
        .join(ACTIVE_FILE)
}

pub fn set_publish_active(active: bool) -> Result<()> {
    let user_dir = kimi_user_dir();
    let path = active_flag_path(&user_dir);
    if active {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(
            &path,
            serde_json::to_string_pretty(&json!({
                "active": true,
                "updatedAt": chrono_like_now(),
            }))?
            .as_bytes(),
        )?;
    } else if path.exists() {
        let _ = fs::remove_file(&path);
    }
    Ok(())
}

pub fn is_publish_active(user_dir: &Path) -> bool {
    let path = active_flag_path(user_dir);
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("active").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn count_spur_models_in_cache(user_dir: &Path) -> Option<u32> {
    let raw = fs::read_to_string(cache_path(user_dir)).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let mut n = 0u32;
    if let Some(root) = value.as_object() {
        for entry in root.values() {
            if let Some(list) = entry.get("models").and_then(Value::as_array) {
                for item in list {
                    let key = item.get("key").and_then(Value::as_str).unwrap_or("");
                    let alias = item.get("modelAlias").and_then(Value::as_str).unwrap_or("");
                    if key.starts_with("spur-") || alias.starts_with("spur-") {
                        n += 1;
                    }
                }
            }
        }
    }
    Some(n)
}

fn opaque_alias(route_slug: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(route_slug.as_bytes());
    let hex = hex::encode(hasher.finalize());
    format!("spur-{}", &hex[..12])
}

fn plan_models(
    catalog: &crate::domain::ModelsResponse,
    routes: &std::collections::HashMap<String, RouteTarget>,
) -> Vec<PlannedModel> {
    let mut out = Vec::new();
    for model in &catalog.models {
        let slug = model.slug.clone();
        let target = routes.get(&slug);
        let display = model.display_name.clone();
        let kind = target
            .map(|t| t.kind.clone())
            .unwrap_or_else(|| "unknown".into());
        let upstream = target
            .map(|t| t.upstream_model.clone())
            .unwrap_or_else(|| slug.clone());
        let alias = opaque_alias(&slug);
        let description = format!("Codex Spur · {kind} · {upstream}");
        out.push(PlannedModel {
            alias,
            display_name: display,
            description,
            route_slug: slug,
            upstream_model: upstream,
            provider_kind: kind,
        });
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    out
}

fn selector_item(model: &PlannedModel) -> Value {
    json!({
        "key": model.alias,
        "displayName": model.display_name,
        "description": model.description,
        "inputPlaceholder": format!("通过 Spur 路由到 {}…", model.upstream_model),
        "label": ["Spur", "实验"],
        "scenario": 6,
        "modelId": model.alias,
        "modelAlias": model.alias,
        "agentMode": "TYPE_NORMAL",
        "kimiPlusId": "ok-computer",
        "switchableTo": [],
        "reasoningEffortOptions": [
            {
                "effort": "REASONING_EFFORT_LOW",
                "reasoningEffort": "low",
                "displayName": "标准",
                "description": "",
                "thinkingLevel": "low"
            },
            {
                "effort": "REASONING_EFFORT_HIGH",
                "reasoningEffort": "high",
                "displayName": "进阶",
                "description": "",
                "thinkingLevel": "high"
            },
            {
                "effort": "REASONING_EFFORT_MAX",
                "reasoningEffort": "max",
                "displayName": "极致",
                "description": "消耗额度更快",
                "thinkingLevel": "max"
            }
        ],
        "defaultReasoningEffort": "REASONING_EFFORT_HIGH",
        "defaultThinkingLevel": "high",
        "daimonModelConfig": {
            "maxContextSize": 262144,
            "capabilities": ["thinking", "tool_use"],
            "supportEfforts": ["low", "high", "max"],
            "defaultEffort": "high",
            "protocol": "",
            "betaApi": false,
            "reasoningKey": ""
        },
        "contextLengthOptions": [
            {
                "contextLength": "CONTEXT_LENGTH_L",
                "displayName": "标准",
                "description": "",
                "label": "",
                "available": true,
                "minMembershipLevel": ""
            }
        ],
        "defaultContextLength": "CONTEXT_LENGTH_L",
        // Spur bookkeeping (ignored by Kimi if unknown)
        "spurRouteSlug": model.route_slug,
        "spurUpstreamModel": model.upstream_model,
        "spurProviderKind": model.provider_kind
    })
}

fn daimon_model_entry(model: &PlannedModel) -> Value {
    json!({
        "provider": SPUR_PROVIDER_ID,
        "model": model.alias,
        "maxContextSize": 262144,
        "capabilities": ["thinking", "tool_use"],
        "supportEfforts": ["low", "high", "max"],
        "defaultEffort": "high"
    })
}

fn toml_escape(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn build_spur_toml_fragment(models: &[PlannedModel], gateway_base: &str, local_token: &str) -> String {
    let mut lines = Vec::new();
    lines.push("".to_string());
    lines.push("# --- Codex Spur (experimental Kimi target) ---".to_string());
    lines.push(format!("[providers.{SPUR_PROVIDER_ID}]"));
    lines.push(format!("type = {SPUR_PROVIDER_TYPE:?}"));
    lines.push(format!("api_key = {}", toml_escape(local_token)));
    lines.push(format!("base_url = {}", toml_escape(gateway_base)));
    lines.push("".to_string());
    for m in models {
        lines.push(format!("[models.{}]", m.alias));
        lines.push(format!("provider = {SPUR_PROVIDER_ID:?}"));
        lines.push(format!("model = {}", toml_escape(&m.alias)));
        lines.push("max_context_size = 262144".into());
        lines.push("capabilities = [ \"thinking\", \"tool_use\" ]".into());
        lines.push("support_efforts = [ \"low\", \"high\", \"max\" ]".into());
        lines.push("default_effort = \"high\"".into());
        lines.push("".to_string());
    }
    lines.push("# --- end Codex Spur ---".to_string());
    lines.join("\n")
}

fn strip_spur_toml_section(toml: &str) -> String {
    let start = "# --- Codex Spur (experimental Kimi target) ---";
    let end = "# --- end Codex Spur ---";
    if let (Some(s), Some(e)) = (toml.find(start), toml.find(end)) {
        let mut out = String::new();
        out.push_str(toml[..s].trim_end());
        out.push('\n');
        let after = &toml[e + end.len()..];
        out.push_str(after.trim_start_matches('\n'));
        return out;
    }
    // Fallback: drop [providers.spur-gateway] and [models.spur-*]
    let mut out = String::new();
    let mut skip = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed == format!("[providers.{SPUR_PROVIDER_ID}]")
            || trimmed.starts_with("[models.spur-")
        {
            skip = true;
            continue;
        }
        if skip {
            if trimmed.starts_with('[') {
                skip = false;
            } else {
                continue;
            }
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn merge_runtime_toml(existing: &str, models: &[PlannedModel], gateway_base: &str, token: &str) -> String {
    let base = strip_spur_toml_section(existing);
    let mut out = base.trim_end().to_string();
    out.push('\n');
    out.push_str(&build_spur_toml_fragment(models, gateway_base, token));
    out.push('\n');
    out
}

fn merge_config_json(
    existing: &Value,
    models: &[PlannedModel],
    gateway_base: &str,
    local_token: &str,
) -> Result<Value> {
    let mut config = existing.clone();
    if !config.is_object() {
        bail!("daimon config.json 不是对象");
    }
    let root = config.as_object_mut().unwrap();

    // credentials.spurGateway
    {
        let credentials = root
            .entry("credentials")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("credentials 不是对象"))?;
        credentials.insert(
            "spurGateway".into(),
            json!({
                "apiKey": local_token,
                "baseUrl": gateway_base
            }),
        );
    }

    let model_root = root
        .entry("model")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("model 不是对象"))?;

    let providers = model_root
        .entry("providers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("model.providers 不是对象"))?;
    providers.insert(
        SPUR_PROVIDER_ID.into(),
        json!({
            "type": SPUR_PROVIDER_TYPE,
            "baseUrl": gateway_base,
            "credential": "spurGateway"
        }),
    );

    let models_map = model_root
        .entry("models")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("model.models 不是对象"))?;

    // Remove previous spur-* model keys
    let stale: Vec<String> = models_map
        .keys()
        .filter(|k| k.starts_with("spur-"))
        .cloned()
        .collect();
    for k in stale {
        models_map.remove(&k);
    }
    for m in models {
        models_map.insert(m.alias.clone(), daimon_model_entry(m));
    }

    Ok(config)
}

#[allow(dead_code)] // used by uninstall_spur_bits (manual/clean path)
fn restore_config_json_remove_spur(existing: &Value) -> Result<Value> {
    let mut config = existing.clone();
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("config 不是对象"))?;
    if let Some(credentials) = root.get_mut("credentials").and_then(|v| v.as_object_mut()) {
        credentials.remove("spurGateway");
    }
    if let Some(model_root) = root.get_mut("model").and_then(|v| v.as_object_mut()) {
        if let Some(providers) = model_root
            .get_mut("providers")
            .and_then(|v| v.as_object_mut())
        {
            providers.remove(SPUR_PROVIDER_ID);
        }
        if let Some(models_map) = model_root.get_mut("models").and_then(|v| v.as_object_mut()) {
            let stale: Vec<String> = models_map
                .keys()
                .filter(|k| k.starts_with("spur-"))
                .cloned()
                .collect();
            for k in stale {
                models_map.remove(&k);
            }
        }
    }
    Ok(config)
}

fn user_id_from_config(config: &Value) -> Option<String> {
    config
        .pointer("/credentials/kimiWeb/userId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn merge_models_cache(
    existing: &Value,
    user_id: Option<&str>,
    models: &[PlannedModel],
) -> Result<Value> {
    let mut cache = if existing.is_object() {
        existing.clone()
    } else {
        json!({})
    };
    let root = cache
        .as_object_mut()
        .ok_or_else(|| anyhow!("models cache 不是对象"))?;

    let key = match user_id {
        Some(uid) if !uid.is_empty() => format!("latest:{uid}"),
        _ => root
            .keys()
            .find(|k| k.starts_with("latest:"))
            .cloned()
            .unwrap_or_else(|| "latest:spur".into()),
    };

    let entry = root.entry(key).or_insert_with(|| json!({"models": []}));
    let entry_obj = entry
        .as_object_mut()
        .ok_or_else(|| anyhow!("cache entry 不是对象"))?;
    let list = entry_obj
        .entry("models")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow!("cache.models 不是数组"))?;

    // Drop previous spur entries
    list.retain(|item| {
        item.get("key")
            .and_then(Value::as_str)
            .map(|k| !k.starts_with("spur-"))
            .unwrap_or(true)
    });
    for m in models {
        list.push(selector_item(m));
    }
    Ok(cache)
}

#[allow(dead_code)] // used by uninstall_spur_bits (manual/clean path)
fn strip_spur_from_cache(existing: &Value) -> Result<Value> {
    let mut cache = existing.clone();
    let root = cache
        .as_object_mut()
        .ok_or_else(|| anyhow!("cache 不是对象"))?;
    for (_k, entry) in root.iter_mut() {
        if let Some(list) = entry
            .get_mut("models")
            .and_then(|v| v.as_array_mut())
        {
            list.retain(|item| {
                item.get("key")
                    .and_then(Value::as_str)
                    .map(|k| !k.starts_with("spur-"))
                    .unwrap_or(true)
            });
        }
    }
    Ok(cache)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    {
        let mut file = fs::File::create(&temp)
            .with_context(|| format!("写入临时文件失败：{}", temp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp, path).with_context(|| format!("原子替换失败：{}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn copy_backup(src: &Path, backup_dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    if !src.exists() {
        return Ok(None);
    }
    fs::create_dir_all(backup_dir)?;
    let dest = backup_dir.join(name);
    fs::copy(src, &dest)
        .with_context(|| format!("备份失败 {} → {}", src.display(), dest.display()))?;
    Ok(Some(dest))
}

fn gateway_coding_base(proxy_base_url: &str) -> String {
    // proxy_base_url is like http://127.0.0.1:17861/v1
    // Kimi type=kimi provider expects …/coding/v1 (agent-gw style).
    let trimmed = proxy_base_url.trim_end_matches('/');
    let root = trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    format!("{root}/coding/v1")
}

fn collect_warnings(status: &KimiTargetStatus, models: &[PlannedModel]) -> Vec<String> {
    let mut warnings = status.warnings.clone();
    if models.is_empty() {
        warnings.push("没有已启用的 Spur 模型路由可发布。".into());
    }
    if !status.control_ready {
        warnings.push(
            "daimon control 未就绪：发布后可等 Kimi 就绪再点「重新推送列表」。".into(),
        );
    }
    warnings.push(
        "仅写盘不会改在线右下角：请在发布后用路径拦截 DescribeKimiWorkConfig（勿拦 agent-gw），再完全退出重开 Kimi。脚本：scripts/kimi_block_work_model_config.py"
            .into(),
    );
    warnings
}

pub fn preview(
    proxy_base_url: &str,
    _local_token: &str,
    catalog: &crate::domain::ModelsResponse,
    routes: &std::collections::HashMap<String, RouteTarget>,
) -> Result<KimiPublishPreview> {
    let status = inspect_status();
    let user_dir = kimi_user_dir();
    let models = plan_models(catalog, routes);
    let gateway = gateway_coding_base(proxy_base_url);
    let labels: Vec<String> = models.iter().map(|m| m.display_name.clone()).collect();
    let warnings = collect_warnings(&status, &models);

    let cache_p = cache_path(&user_dir);
    let config_p = config_path(&user_dir);
    let toml_p = runtime_toml_path(&user_dir);

    let existing_cache: Value = fs::read_to_string(&cache_p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let existing_config: Value = fs::read_to_string(&config_p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let uid = user_id_from_config(&existing_config);
    let merged_cache = merge_models_cache(&existing_cache, uid.as_deref(), &models)?;
    let cache_preview = serde_json::to_string_pretty(&json!({
        "spurModels": models.iter().map(|m| json!({
            "alias": m.alias,
            "displayName": m.display_name,
            "route": m.route_slug
        })).collect::<Vec<_>>(),
        "cacheKeys": merged_cache.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
    }))?;

    Ok(KimiPublishPreview {
        experimental: true,
        kimi_version: status.app_version,
        gateway_base_url: gateway,
        model_count: models.len() as u32,
        model_labels: labels,
        cache_path: cache_p.display().to_string(),
        config_path: config_p.display().to_string(),
        runtime_toml_path: toml_p.display().to_string(),
        cache_preview,
        config_diff_summary: format!(
            "upsert credentials.spurGateway + model.providers.{SPUR_PROVIDER_ID} + {} models",
            models.len()
        ),
        toml_diff_summary: format!(
            "append [providers.{SPUR_PROVIDER_ID}] and {} [models.spur-*] blocks",
            models.len()
        ),
        warnings,
    })
}

pub fn apply(
    proxy_base_url: &str,
    local_token: &str,
    catalog: &crate::domain::ModelsResponse,
    routes: &std::collections::HashMap<String, RouteTarget>,
) -> Result<KimiPublishOutcome> {
    let status = inspect_status();
    let user_dir = kimi_user_dir();
    if !user_dir.exists() {
        bail!("Kimi 用户目录不存在：{}（请先启动 Kimi Desktop）", user_dir.display());
    }
    let models = plan_models(catalog, routes);
    if models.is_empty() {
        bail!("没有可发布的模型路由；请先在 Spur 启用模型。");
    }
    let gateway = gateway_coding_base(proxy_base_url);
    let mut warnings = collect_warnings(&status, &models);

    let cache_p = cache_path(&user_dir);
    let config_p = config_path(&user_dir);
    let toml_p = runtime_toml_path(&user_dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_dir = spur_backup_root(&user_dir).join(format!("backup-{stamp}"));
    fs::create_dir_all(&backup_dir)?;
    copy_backup(&cache_p, &backup_dir, "kimi-work-models-cache.json")?;
    copy_backup(&config_p, &backup_dir, "config.json")?;
    copy_backup(&toml_p, &backup_dir, "config.toml")?;

    let existing_cache: Value = fs::read_to_string(&cache_p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let existing_config: Value = {
        let raw = fs::read_to_string(&config_p).context("读取 daimon config.json 失败")?;
        serde_json::from_str(&raw).context("解析 daimon config.json 失败")?
    };
    let existing_toml = fs::read_to_string(&toml_p).unwrap_or_default();
    let uid = user_id_from_config(&existing_config);

    let next_cache = merge_models_cache(&existing_cache, uid.as_deref(), &models)?;
    let next_config = merge_config_json(&existing_config, &models, &gateway, local_token)?;
    let next_toml = merge_runtime_toml(&existing_toml, &models, &gateway, local_token);

    let cache_bytes = serde_json::to_vec_pretty(&next_cache)?;
    let config_bytes = serde_json::to_vec_pretty(&next_config)?;
    let toml_bytes = next_toml.as_bytes();

    atomic_write(&cache_p, &cache_bytes)?;
    atomic_write(&config_p, &config_bytes)?;
    atomic_write(&toml_p, toml_bytes)?;

    let labels: Vec<String> = models.iter().map(|m| m.display_name.clone()).collect();
    let aliases: Vec<String> = models.iter().map(|m| m.alias.clone()).collect();
    let published_at = chrono_like_now();
    let marker = PublishMarker {
        published_at: published_at.clone(),
        model_count: models.len() as u32,
        model_aliases: aliases.clone(),
        gateway_base_url: gateway,
        kimi_version: status.app_version.clone(),
        backup_dir: backup_dir.display().to_string(),
    };
    atomic_write(
        &marker_path(&user_dir),
        serde_json::to_string_pretty(&marker)?.as_bytes(),
    )?;

    // Also write alias → route map next to marker for the local gateway.
    let mut alias_map = BTreeMap::new();
    for m in &models {
        alias_map.insert(m.alias.clone(), m.route_slug.clone());
    }
    let map_path = user_dir
        .join("daimon-share/daimon")
        .join(".codex-spur-kimi-alias-map.json");
    atomic_write(&map_path, serde_json::to_string_pretty(&alias_map)?.as_bytes())?;

    // Verify cache actually contains spur keys after write.
    let cache_spur = count_spur_models_in_cache(&user_dir).unwrap_or(0);
    if cache_spur == 0 {
        warnings.push("写入后 cache 仍无 spur-*，发布可能未生效。".into());
    } else {
        warnings.push(format!(
            "已写入 cache：{cache_spur} 个 spur-*。下一步：启用路径拦截 → 完全退出并重开 Kimi。"
        ));
    }

    let control_updated = push_models_to_daimon_control_retry(&user_dir, &models, 3);
    if !control_updated {
        warnings.push(
            "control RPC 未成功（可忽略：右下角主要靠 cache + 拦 DescribeKimiWorkConfig）。".into(),
        );
    }

    warnings.push(
        "勿拦 agent-gw.kimi.com / 整站 www.kimi.com；只拦 …/ConfigService/DescribeKimiWorkConfig。"
            .into(),
    );

    let _ = content_hash(&cache_bytes);

    Ok(KimiPublishOutcome {
        experimental: true,
        model_count: models.len() as u32,
        model_labels: labels,
        backup_dir: backup_dir.display().to_string(),
        cache_path: cache_p.display().to_string(),
        config_path: config_p.display().to_string(),
        runtime_toml_path: toml_p.display().to_string(),
        control_updated,
        restart_recommended: true,
        warnings,
    })
}

/// Retry control inject a few times — daimon is often "provisioning" right after launch.
fn push_models_to_daimon_control_retry(user_dir: &Path, models: &[PlannedModel], attempts: u32) -> bool {
    for i in 0..attempts {
        match push_models_to_daimon_control(user_dir, models) {
            Ok(true) => return true,
            Ok(false) | Err(_) => {
                if i + 1 < attempts {
                    std::thread::sleep(std::time::Duration::from_millis(400 * (i as u64 + 1)));
                }
            }
        }
    }
    false
}

fn chrono_like_now() -> String {
    // Avoid extra chrono dep: RFC3339-ish UTC from system time is fine for marker.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Best-effort JSON-RPC over daimon control WebSocket.
/// Uses a tiny raw handshake so we don't add a websocket crate if unavailable —
/// we shell out via `curl` is not viable for WS; implement minimal client.
fn push_models_to_daimon_control(user_dir: &Path, models: &[PlannedModel]) -> Result<bool> {
    let Some((url, token)) = control_endpoint(user_dir) else {
        return Ok(false);
    };
    // Minimal WS client implemented below with std::net + manual handshake.
    // Note: Work UI picker is driven by cloud DescribeKimiWorkConfig, not this list.
    // Control inject still helps daimon route resolution for spur aliases when selected.
    let payload_models: Vec<Value> = models
        .iter()
        .map(|m| {
            json!({
                "model": m.alias,
                "maxContextSize": 262144,
                "capabilities": ["thinking", "tool_use"],
                "supportEfforts": ["low", "high", "max"],
                "defaultEffort": "high"
            })
        })
        .collect();

    let get_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "conversations.getKimiModelList",
        "params": {}
    });

    let mut merged = payload_models.clone();
    if let Ok(current) = ws_jsonrpc_call(&url, &token, &get_req) {
        if let Some(arr) = current.pointer("/result/models").and_then(|v| v.as_array()) {
            for item in arr {
                let model = item.get("model").and_then(Value::as_str).unwrap_or("");
                if model.is_empty() || model.starts_with("spur-") {
                    continue;
                }
                if !merged
                    .iter()
                    .any(|m| m.get("model").and_then(Value::as_str) == Some(model))
                {
                    merged.push(item.clone());
                }
            }
        }
    }

    let update_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "conversations.updateKimiModelList",
        "params": { "models": merged }
    });
    ws_jsonrpc_call(&url, &token, &update_req)?;
    Ok(true)
}

fn ws_jsonrpc_call(url: &str, token: &str, request: &Value) -> Result<Value> {
    // Parse ws://host:port/path
    let url = url
        .strip_prefix("ws://")
        .ok_or_else(|| anyhow!("仅支持 ws:// control URL"))?;
    let (hostport, path) = url
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((url, "/".into()));
    let (host, port) = if let Some((h, p)) = hostport.split_once(':') {
        (h, p.parse::<u16>().unwrap_or(80))
    } else {
        (hostport, 80)
    };

    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect((host, port))
        .with_context(|| format!("连接 control 失败 {host}:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let key = base64_simple(&random_16());
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    // Also try token query/header variants used by loopback-dev-token
    let handshake = format!(
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n{auth_header}Sec-WebSocket-Protocol: daimon.control.v1\r\n\r\n"
    );
    stream.write_all(handshake.as_bytes())?;

    let mut header_buf = Vec::new();
    let mut tmp = [0u8; 1];
    while header_buf.len() < 8192 {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        header_buf.push(tmp[0]);
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_text = String::from_utf8_lossy(&header_buf);
    if !header_text.starts_with("HTTP/1.1 101") && !header_text.contains("101") {
        // Retry without subprotocol / with token query
        return ws_jsonrpc_call_token_query(host, port, hostport, &path, token, request);
    }

    write_ws_text(&mut stream, &serde_json::to_string(request)?)?;
    let body = read_ws_text(&mut stream)?;
    serde_json::from_str(&body).context("解析 control JSON-RPC 响应失败")
}

fn ws_jsonrpc_call_token_query(
    host: &str,
    port: u16,
    hostport: &str,
    path: &str,
    token: &str,
    request: &Value,
) -> Result<Value> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let key = base64_simple(&random_16());
    let sep = if path.contains('?') { "&" } else { "?" };
    let full_path = format!("{path}{sep}token={token}");
    let handshake = format!(
        "GET {full_path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(handshake.as_bytes())?;
    let mut header_buf = Vec::new();
    let mut tmp = [0u8; 1];
    while header_buf.len() < 8192 {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        header_buf.push(tmp[0]);
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_text = String::from_utf8_lossy(&header_buf);
    if !header_text.contains("101") {
        bail!("WebSocket upgrade 失败：{}", header_text.lines().next().unwrap_or(""));
    }
    write_ws_text(&mut stream, &serde_json::to_string(request)?)?;
    let body = read_ws_text(&mut stream)?;
    serde_json::from_str(&body).context("解析 control JSON-RPC 响应失败")
}

fn random_16() -> [u8; 16] {
    let mut buf = [0u8; 16];
    let _ = getrandom::fill(&mut buf);
    buf
}

fn base64_simple(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn write_ws_text(stream: &mut impl Write, text: &str) -> Result<()> {
    let payload = text.as_bytes();
    let mut frame = Vec::new();
    frame.push(0x81); // FIN + text
    let mask_bit = 0x80;
    if payload.len() < 126 {
        frame.push(mask_bit | payload.len() as u8);
    } else if payload.len() <= 65535 {
        frame.push(mask_bit | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(mask_bit | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    let mask = random_16();
    let mask = [mask[0], mask[1], mask[2], mask[3]];
    frame.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        frame.push(b ^ mask[i % 4]);
    }
    stream.write_all(&frame)?;
    Ok(())
}

fn read_ws_text(stream: &mut impl Read) -> Result<String> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr)?;
    let opcode = hdr[0] & 0x0f;
    if opcode == 0x8 {
        bail!("control websocket closed");
    }
    let mut len = (hdr[1] & 0x7f) as u64;
    let masked = (hdr[1] & 0x80) != 0;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext)?;
        len = u16::from_be_bytes(ext) as u64;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext)?;
        len = u64::from_be_bytes(ext);
    }
    if len > 4_000_000 {
        bail!("control 响应过大");
    }
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    String::from_utf8(payload).context("control 响应不是 UTF-8")
}

pub fn restore_latest() -> Result<Option<String>> {
    let user_dir = kimi_user_dir();
    let root = spur_backup_root(&user_dir);
    if !root.exists() {
        return Ok(None);
    }
    let mut backups: Vec<PathBuf> = fs::read_dir(&root)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    backups.sort();
    let Some(latest) = backups.pop() else {
        return Ok(None);
    };

    let cache_src = latest.join("kimi-work-models-cache.json");
    let config_src = latest.join("config.json");
    let toml_src = latest.join("config.toml");
    if cache_src.exists() {
        atomic_write(&cache_path(&user_dir), &fs::read(&cache_src)?)?;
    }
    if config_src.exists() {
        atomic_write(&config_path(&user_dir), &fs::read(&config_src)?)?;
    }
    if toml_src.exists() {
        atomic_write(&runtime_toml_path(&user_dir), &fs::read(&toml_src)?)?;
    }
    let _ = fs::remove_file(marker_path(&user_dir));
    let _ = fs::remove_file(
        user_dir
            .join("daimon-share/daimon")
            .join(".codex-spur-kimi-alias-map.json"),
    );
    Ok(Some(latest.display().to_string()))
}

/// Remove Spur injections without requiring a backup (best-effort clean).
pub fn uninstall_spur_bits() -> Result<()> {
    let user_dir = kimi_user_dir();
    let config_p = config_path(&user_dir);
    let cache_p = cache_path(&user_dir);
    let toml_p = runtime_toml_path(&user_dir);
    if config_p.exists() {
        let existing: Value = serde_json::from_str(&fs::read_to_string(&config_p)?)?;
        let next = restore_config_json_remove_spur(&existing)?;
        atomic_write(&config_p, serde_json::to_vec_pretty(&next)?.as_slice())?;
    }
    if cache_p.exists() {
        let existing: Value = serde_json::from_str(&fs::read_to_string(&cache_p)?)?;
        let next = strip_spur_from_cache(&existing)?;
        atomic_write(&cache_p, serde_json::to_vec_pretty(&next)?.as_slice())?;
    }
    if toml_p.exists() {
        let existing = fs::read_to_string(&toml_p)?;
        let next = strip_spur_toml_section(&existing);
        atomic_write(&toml_p, next.as_bytes())?;
    }
    let _ = fs::remove_file(marker_path(&user_dir));
    Ok(())
}

/// Load alias → spur-route map written by apply().
pub fn load_alias_route_map() -> BTreeMap<String, String> {
    let path = kimi_user_dir()
        .join("daimon-share/daimon")
        .join(".codex-spur-kimi-alias-map.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_alias_is_stable_and_short() {
        let a = opaque_alias("spur-route-abc");
        let b = opaque_alias("spur-route-abc");
        assert_eq!(a, b);
        assert!(a.starts_with("spur-"));
        assert_eq!(a.len(), "spur-".len() + 12);
    }

    #[test]
    fn strip_and_merge_toml_roundtrip() {
        let base = "default_model = \"k3-agent\"\n\n[providers.daimon-kimi-code]\ntype = \"kimi\"\n";
        let models = vec![PlannedModel {
            alias: "spur-deadbeefcafe".into(),
            display_name: "X".into(),
            description: "d".into(),
            route_slug: "r".into(),
            upstream_model: "m".into(),
            provider_kind: "xai".into(),
        }];
        let merged = merge_runtime_toml(base, &models, "http://127.0.0.1:17861/coding/v1", "tok");
        assert!(merged.contains("[providers.spur-gateway]"));
        assert!(merged.contains("[models.spur-deadbeefcafe]"));
        let stripped = strip_spur_toml_section(&merged);
        assert!(!stripped.contains("spur-gateway"));
        assert!(stripped.contains("daimon-kimi-code"));
    }

    #[test]
    fn merge_cache_appends_spur_models() {
        let existing = json!({
            "latest:u1": {
                "models": [{
                    "key": "k3-agent",
                    "displayName": "K3",
                    "modelAlias": "k3-agent"
                }]
            }
        });
        let models = vec![PlannedModel {
            alias: "spur-aaaabbbbcccc".into(),
            display_name: "Grok".into(),
            description: "d".into(),
            route_slug: "spur-route-x".into(),
            upstream_model: "grok-3".into(),
            provider_kind: "xai".into(),
        }];
        let merged = merge_models_cache(&existing, Some("u1"), &models).unwrap();
        let list = merged["latest:u1"]["models"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|m| m["key"] == "spur-aaaabbbbcccc"));
        assert!(list.iter().any(|m| m["key"] == "k3-agent"));
    }

    #[test]
    fn gateway_coding_base_strips_v1() {
        assert_eq!(
            gateway_coding_base("http://127.0.0.1:17861/v1"),
            "http://127.0.0.1:17861/coding/v1"
        );
    }
}
