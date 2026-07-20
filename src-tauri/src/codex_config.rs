use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::domain::{ApplyPreview, ModelsResponse, ReasoningEffort};

/// Env-aware Codex home (may be hijacked by Orca/Herdr via `CODEX_HOME`).
/// Prefer [`publish_codex_home`] for ChatGPT GUI integration.
pub fn codex_home() -> PathBuf {
    if let Some(value) = env::var_os("CODEX_HOME") {
        return PathBuf::from(value);
    }
    user_codex_home()
}

/// Real user Codex home ChatGPT.app reads: `$HOME/.codex`.
///
/// Herdr/Orca set `CODEX_HOME` to an isolated sandbox. Publishing there makes
/// Spur report success while the GUI still loads CC Switch from `~/.codex`.
/// Override only for tests via `CODEX_SPUR_PUBLISH_HOME`.
pub fn publish_codex_home() -> PathBuf {
    if let Some(value) = env::var_os("CODEX_SPUR_PUBLISH_HOME") {
        return PathBuf::from(value);
    }
    user_codex_home()
}

fn user_codex_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".codex"))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub fn catalog_path_for(home: &Path) -> PathBuf {
    home.join("codex-select").join("model-catalog.json")
}

pub fn config_path_for(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// Live binding state as ChatGPT would see it under the publish home.
#[derive(Debug, Clone)]
pub struct LiveBinding {
    pub state: String,
    pub codex_home: PathBuf,
    pub provider_id: String,
    pub catalog_path: PathBuf,
    pub attention: Vec<String>,
}

pub fn inspect_live_binding() -> LiveBinding {
    let home = publish_codex_home();
    let catalog_path = catalog_path_for(&home);
    let config_path = config_path_for(&home);
    let mut attention = Vec::new();

    // CODEX_HOME isolation (Orca/Herdr) is informational only when we already publish
    // to the real ~/.codex path ChatGPT reads. Do not put it in "需要处理" — it does
    // not block routing and confuses users after a successful apply.
    let _ = env_codex_home_diverges(&home);

    if !chatgpt_auth_present(&home) {
        attention.push(
            "未检测到有效的 ChatGPT 官方登录（~/.codex/auth.json）。Desktop 会隐藏 Kimi/DeepSeek 等自定义模型，只显示官方 GPT-5.6。请在 ChatGPT 中登录一次官方账号，再 Cmd+Q 完全退出后重开。"
                .into(),
        );
    }

    let Ok(raw) = fs::read_to_string(&config_path) else {
        return LiveBinding {
            state: "not_applied".into(),
            codex_home: home,
            provider_id: "codex_select".into(),
            catalog_path,
            attention,
        };
    };
    let Ok(document) = raw.parse::<DocumentMut>() else {
        attention.push("Codex config.toml 无法解析，请先修复配置。".into());
        return LiveBinding {
            state: "invalid".into(),
            codex_home: home,
            provider_id: "codex_select".into(),
            catalog_path,
            attention,
        };
    };

    let provider = document
        .get("model_provider")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    let catalog_ref = document
        .get("model_catalog_json")
        .and_then(|item| item.as_str())
        .unwrap_or("");

    if provider == "codex_select" {
        let expected = catalog_path.display().to_string();
        let catalog_ok = catalog_ref == expected
            || Path::new(catalog_ref)
                .file_name()
                .is_some_and(|name| name == "model-catalog.json")
                && catalog_ref.contains("codex-select");
        if catalog_ok && catalog_path.exists() {
            if hooks_mention_cc_switch(&home) {
                attention.push(
                    "SessionStart 仍注册了 CC Switch 同步脚本；若 CC Switch 再次设为 current，可能覆盖 model_provider。"
                        .into(),
                );
            }
            return LiveBinding {
                state: "applied".into(),
                codex_home: home,
                provider_id: "codex_select".into(),
                catalog_path,
                attention,
            };
        }
        attention.push("model_provider 已是 codex_select，但 catalog 路径异常。".into());
        return LiveBinding {
            state: "invalid".into(),
            codex_home: home,
            provider_id: "codex_select".into(),
            catalog_path,
            attention,
        };
    }

    if provider == "custom" || catalog_ref.contains("cc-switch") {
        attention.push(
            "Codex 仍绑定 CC Switch（custom / cc-switch-model-catalog）。模型列表不会显示 Spur/Kimi，请点击 Review & Apply。"
                .into(),
        );
    } else if !provider.is_empty() && provider != "codex_select" {
        attention.push(format!(
            "Codex 当前 model_provider = \"{provider}\"，尚未切换到 codex_select。"
        ));
    }

    if hooks_mention_cc_switch(&home) {
        attention.push(
            "SessionStart 仍注册了 CC Switch 同步脚本；应用后若被 CC Switch 抢回，请勿再切换其 current 供应商。"
                .into(),
        );
    }

    LiveBinding {
        state: "not_applied".into(),
        codex_home: home,
        provider_id: "codex_select".into(),
        catalog_path,
        attention,
    }
}

fn env_codex_home_diverges(publish_home: &Path) -> bool {
    env::var_os("CODEX_HOME").is_some_and(|value| {
        let env_home = PathBuf::from(value);
        env_home != publish_home
    })
}

/// True when ~/.codex/auth.json looks like a ChatGPT/Codex OAuth login (not API-key only).
/// Used for Desktop custom-model visibility diagnostics — never reads token bodies into logs.
fn chatgpt_auth_present(home: &Path) -> bool {
    let path = home.join("auth.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let mode = value
        .get("auth_mode")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if mode == "chatgpt" || mode == "codex" {
        return value.get("tokens").is_some();
    }
    // Legacy shapes: tokens object with refresh/access is enough for Desktop gating.
    value
        .get("tokens")
        .and_then(|tokens| tokens.as_object())
        .is_some_and(|tokens| {
            tokens.contains_key("access_token") || tokens.contains_key("refresh_token")
        })
}

fn hooks_mention_cc_switch(home: &Path) -> bool {
    let hooks = home.join("hooks.json");
    fs::read_to_string(hooks)
        .map(|text| text.contains("cc-switch-routing") || text.contains("session_start_sync"))
        .unwrap_or(false)
}

pub fn preview(base_url: &str, model_count: u32) -> ApplyPreview {
    let home = publish_codex_home();
    let catalog_path = catalog_path_for(&home);
    let config_path = config_path_for(&home);
    let toml_preview = format!(
        r#"model_provider = "codex_select"
model_catalog_json = "{}"

[model_providers.codex_select]
name = "OpenAI"
base_url = "{}"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false"#,
        catalog_path.display(),
        base_url,
    );
    let mut warnings = Vec::new();
    if model_count == 0 {
        warnings.push("当前没有已选择模型，应用会被阻止。".into());
    }
    if !config_path.exists() {
        warnings.push(format!("尚未找到 Codex 配置：{}", config_path.display()));
    }
    if env_codex_home_diverges(&home) {
        warnings.push(format!(
            "CODEX_HOME={} 与 ChatGPT 使用的 {} 不同；将写入后者。",
            codex_home().display(),
            home.display()
        ));
    }
    let live = inspect_live_binding();
    warnings.extend(live.attention);
    ApplyPreview {
        provider_id: "codex_select".into(),
        base_url: base_url.into(),
        catalog_path: catalog_path.display().to_string(),
        selected_model: None,
        model_count,
        toml_preview,
        warnings,
    }
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub catalog_path: PathBuf,
    pub config_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub before_hash: Option<String>,
    pub after_hash: String,
    pub model_count: u32,
    pub selected_model: Option<String>,
    pub model_labels: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn apply(base_url: &str, bearer_token: &str, catalog: &ModelsResponse) -> Result<ApplyResult> {
    if catalog.models.is_empty() {
        anyhow::bail!("至少选择一个模型后才能应用到 Codex");
    }
    crate::catalog::validate_catalog(catalog).context("Codex catalog 校验失败，未写入任何文件")?;
    let catalog_json = serde_json::to_vec_pretty(catalog)?;

    let home = publish_codex_home();
    let mut warnings = Vec::new();

    let select_dir = home.join("codex-select");
    let backup_dir = select_dir.join("backups");
    fs::create_dir_all(&backup_dir).context("无法创建 Codex Spur 配置目录")?;
    let catalog_path = catalog_path_for(&home);
    let config_path = config_path_for(&home);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let original_config = match fs::read_to_string(&config_path) {
        Ok(value) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex config.toml 失败：{}", config_path.display()))
        }
    };
    let original_catalog = match fs::read(&catalog_path) {
        Ok(value) => Some(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex catalog 失败：{}", catalog_path.display()))
        }
    };
    let before_hash = original_config
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| hash_bytes(value.as_bytes()));
    let backup_path = if config_path.exists() {
        let path = backup_dir.join(format!("config-{timestamp}.toml"));
        atomic_write(&path, original_config.as_deref().unwrap_or_default().as_bytes())?;
        Some(path)
    } else {
        None
    };

    let mut document = if original_config.as_deref().unwrap_or_default().trim().is_empty() {
        DocumentMut::new()
    } else {
        original_config
            .as_deref()
            .expect("non-empty config has content")
            .parse::<DocumentMut>()
            .context("Codex config.toml 不是有效 TOML，已停止应用")?
    };

    let selected_model = choose_selected_model(&document, catalog);
    let selected_effort = selected_model
        .as_ref()
        .and_then(|slug| {
            catalog
                .models
                .iter()
                .find(|model| &model.slug == slug)
                .and_then(|model| model.default_reasoning_level)
        })
        .unwrap_or(ReasoningEffort::Medium);

    // ChatGPT's GUI resolves model catalogs relative to CODEX_HOME, matching CC Switch.
    let catalog_path_str = "codex-select/model-catalog.json";
    document["model_provider"] = value("codex_select");
    document["model_catalog_json"] = value(catalog_path_str);
    // Never leave response storage disabled: that empties ChatGPT's local history UX
    // even though session files on disk remain.
    document["disable_response_storage"] = value(false);
    if let Some(slug) = selected_model.as_ref() {
        document["model"] = value(slug.as_str());
        document["model_reasoning_effort"] = value(selected_effort.as_str());
    }

    let providers = document["model_providers"]
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("model_providers 不是 TOML table")?;
    let mut select_provider = Table::new();
    // Desktop model-picker gate (Nice Switch "preserve official login" pattern):
    // ChatGPT.app hides non-official catalog rows (spur-route-*, nice-route-*, DeepSeek,
    // Kimi, …) when the active provider does not present an OpenAI identity. Setting
    // name="OpenAI" + requires_openai_auth=true lets Desktop load ~/.codex/auth.json
    // for identity/gating only. Request auth still uses experimental_bearer_token
    // against the local Spur proxy (base_url) — official tokens are never sent upstream.
    select_provider["name"] = value("OpenAI");
    select_provider["base_url"] = value(base_url);
    select_provider["wire_api"] = value("responses");
    select_provider["requires_openai_auth"] = value(true);
    // Explicit false: Codex defaults may probe websocket /v1/responses and surface
    // multi-retry reconnects before falling back to SSE (Nice/CC Switch pattern).
    select_provider["supports_websockets"] = value(false);
    select_provider["experimental_bearer_token"] = value(bearer_token);
    providers["codex_select"] = Item::Table(select_provider);

    let config_bytes = document.to_string().into_bytes();
    let write_result = (|| -> Result<()> {
        atomic_write(&catalog_path, &catalog_json)?;
        atomic_write(&config_path, &config_bytes)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let rollback_result = rollback_file(&catalog_path, original_catalog.as_deref())
            .and(rollback_file(
                &config_path,
                original_config.as_ref().map(String::as_bytes),
            ));
        return match rollback_result {
            Ok(()) => Err(error).context("Codex catalog/config 写入失败，已回滚") ,
            Err(rollback_error) => Err(error).context(format!(
                "Codex catalog/config 写入失败，且回滚失败：{rollback_error}"
            )),
        };
    }

    // Verify both files under one rollback boundary.
    let verification = (|| -> Result<Vec<u8>> {
        let after_bytes = fs::read(&config_path).context("读取应用后的 Codex 配置失败")?;
        let after_text = String::from_utf8_lossy(&after_bytes);
        let verified = after_text
            .parse::<DocumentMut>()
            .context("应用后的 Codex config.toml 无法解析")?;
        let provider_ok = verified
            .get("model_provider")
            .and_then(|item| item.as_str())
            == Some("codex_select");
        let catalog_ok = verified
            .get("model_catalog_json")
            .and_then(|item| item.as_str())
            .is_some_and(|path| path == catalog_path_str);
        if !provider_ok || !catalog_ok {
            anyhow::bail!(
                "{} 未正确指向 codex_select / {}",
                config_path.display(),
                catalog_path.display()
            );
        }
        let select_table = verified
            .get("model_providers")
            .and_then(|item| item.get("codex_select"));
        let desktop_gate_ok = select_table
            .and_then(|item| item.get("requires_openai_auth"))
            .and_then(|item| item.as_bool())
            == Some(true)
            && select_table
                .and_then(|item| item.get("name"))
                .and_then(|item| item.as_str())
                == Some("OpenAI");
        if !desktop_gate_ok {
            anyhow::bail!(
                "应用后 codex_select 未设置 Desktop 门控字段（name=OpenAI, requires_openai_auth=true）；Kimi/DeepSeek 会被 GUI 隐藏"
            );
        }

        let written_catalog =
            fs::read_to_string(&catalog_path).context("读取应用后的 catalog 失败")?;
        if written_catalog.contains("\"displayName\"")
            || written_catalog.contains("\"supportedReasoningLevels\"")
            || written_catalog.contains("\"experimentalSupportedTools\"")
        {
            anyhow::bail!(
                "应用后 catalog 仍含 camelCase 字段，Codex 会 Invalid configuration 并清空模型列表"
            );
        }
        if !written_catalog.contains("\"experimental_supported_tools\"") {
            anyhow::bail!(
                "应用后 catalog 缺少 experimental_supported_tools（Codex 硬依赖，缺了会解析失败）"
            );
        }
        let parsed_catalog: ModelsResponse =
            serde_json::from_str(&written_catalog).context("应用后的 catalog JSON 无法解析")?;
        if parsed_catalog.models.len() != catalog.models.len() {
            anyhow::bail!(
                "应用后 catalog 模型数量不匹配（期望 {}，实际 {}）",
                catalog.models.len(),
                parsed_catalog.models.len()
            );
        }
        crate::catalog::validate_catalog(&parsed_catalog)
            .context("应用后的 catalog 严格校验失败")?;
        if !after_text.contains("supports_websockets = false") {
            anyhow::bail!("应用后 codex_select provider 未设置 supports_websockets = false");
        }
        Ok(after_bytes)
    })();
    let after_bytes = match verification {
        Ok(bytes) => bytes,
        Err(error) => {
            let rollback_result = rollback_file(&catalog_path, original_catalog.as_deref()).and(
                rollback_file(&config_path, original_config.as_ref().map(String::as_bytes)),
            );
            return match rollback_result {
                Ok(()) => Err(error).context("应用后校验失败，已回滚"),
                Err(rollback_error) => Err(error).context(format!(
                    "应用后校验失败，且回滚失败：{rollback_error}"
                )),
            };
        }
    };

    if hooks_mention_cc_switch(&home) {
        warnings.push(
            "SessionStart 仍注册了 CC Switch 同步脚本；应用后请勿再把 CC Switch 设为 current，否则会把模型列表抢回仅 GPT-5.6 三只。"
                .into(),
        );
    }
    if chatgpt_desktop_running() {
        warnings.push(
            "ChatGPT 仍在运行：模型目录只在启动时加载。请现在 Cmd+Q 完全退出 ChatGPT/Codex，再重新打开，否则菜单不会出现新写入的 Kimi 等模型。"
                .into(),
        );
    }

    let model_labels = catalog
        .models
        .iter()
        .map(|model| model.display_name.clone())
        .collect();

    Ok(ApplyResult {
        catalog_path,
        config_path,
        backup_path,
        before_hash,
        after_hash: hash_bytes(&after_bytes),
        model_count: catalog.models.len() as u32,
        selected_model,
        model_labels,
        warnings,
    })
}

/// Best-effort: true if ChatGPT desktop process is still running (macOS).
fn chatgpt_desktop_running() -> bool {
    let Ok(output) = std::process::Command::new("pgrep")
        .args(["-x", "ChatGPT"])
        .output()
    else {
        return false;
    };
    output.status.success()
}

pub fn restore_latest() -> Result<Option<PathBuf>> {
    let home = publish_codex_home();
    let backup_dir = home.join("codex-select").join("backups");
    let mut backups = fs::read_dir(&backup_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect::<Vec<_>>();
    backups.sort();
    let Some(latest) = backups.pop() else {
        return Ok(None);
    };
    let config_path = config_path_for(&home);
    let bytes = fs::read(&latest).context("读取 Codex 备份失败")?;
    atomic_write(&config_path, &bytes)?;
    Ok(Some(latest))
}

/// Prefer keeping the current model if it is still published; otherwise first sorted slug.
fn choose_selected_model(document: &DocumentMut, catalog: &ModelsResponse) -> Option<String> {
    let current = document
        .get("model")
        .and_then(|item| item.as_str())
        .map(str::to_string);
    if let Some(current) = current.as_ref() {
        if catalog.models.iter().any(|model| &model.slug == current) {
            return Some(current.clone());
        }
        // Legacy path slug: "uuid/gpt-5.6-luna" or bare "gpt-5.6-luna"
        let bare = current.rsplit('/').next().unwrap_or(current.as_str());
        if let Some(model) = catalog.models.iter().find(|model| {
            model.slug == *current
                || model.slug.ends_with(&format!("/{current}"))
                || current.ends_with(&format!("/{}", model.slug))
                || model
                    .display_name
                    .to_ascii_lowercase()
                    .contains(&bare.to_ascii_lowercase())
                || model
                    .description
                    .as_ref()
                    .is_some_and(|text| text.to_ascii_lowercase().contains(&bare.to_ascii_lowercase()))
        }) {
            return Some(model.slug.clone());
        }
    }
    catalog.models.first().map(|model| model.slug.clone())
}

fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, bytes).with_context(|| format!("写入临时文件失败：{}", temp.display()))?;
    let file = fs::OpenOptions::new().read(true).open(&temp)?;
    file.sync_all().with_context(|| format!("刷新临时文件失败：{}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("原子替换文件失败：{}", path.display()))?;
    if let Some(parent) = path.parent() {
        let directory = fs::File::open(parent)?;
        directory.sync_all().with_context(|| format!("刷新目录失败：{}", parent.display()))?;
    }
    Ok(())
}

fn rollback_file(path: &Path, original: Option<&[u8]>) -> Result<()> {
    match original {
        Some(bytes) => atomic_write(&path.to_path_buf(), bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CatalogModel, ReasoningEffortPreset, TruncationPolicy};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn sample_model(slug: &str, name: &str) -> CatalogModel {
        CatalogModel {
            slug: slug.into(),
            display_name: name.into(),
            description: Some("test".into()),
            default_reasoning_level: Some(ReasoningEffort::Medium),
            supported_reasoning_levels: ReasoningEffort::ALL
                .into_iter()
                .map(|effort| ReasoningEffortPreset {
                    effort,
                    description: effort.as_str().into(),
                })
                .collect(),
            shell_type: "shell_command".into(),
            visibility: "list".into(),
            supported_in_api: true,
            priority: 1000,
            additional_speed_tiers: Vec::new(),
            service_tiers: Vec::new(),
            default_service_tier: None,
            availability_nux: None,
            upgrade: None,
            base_instructions: "You are Codex.".into(),
            model_messages: None,
            include_skills_usage_instructions: false,
            supports_reasoning_summaries: true,
            supports_reasoning_summary_parameter: false,
            default_reasoning_summary: serde_json::Value::String("auto".into()),
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
            context_window: Some(128_000),
            max_context_window: Some(128_000),
            auto_compact_token_limit: Some(115_200),
            comp_hash: None,
            effective_context_window_percent: 95,
            experimental_supported_tools: Vec::new(),
            input_modalities: vec!["text".into()],
            supports_search_tool: false,
            use_responses_lite: false,
            auto_review_model_override: None,
            tool_mode: None,
            multi_agent_version: None,
        }
    }

    #[test]
    fn apply_writes_to_publish_home_not_env_codex_home() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let publish_dir = std::env::temp_dir().join(format!(
            "codex-spur-publish-{}-{}",
            std::process::id(),
            stamp
        ));
        let orca_dir = std::env::temp_dir().join(format!(
            "codex-spur-orca-{}-{}",
            std::process::id(),
            stamp
        ));
        fs::create_dir_all(&publish_dir).expect("publish dir");
        fs::create_dir_all(&orca_dir).expect("orca dir");
        // Simulate Orca hijack + test override for publish target.
        std::env::set_var("CODEX_HOME", &orca_dir);
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &publish_dir);

        let catalog = ModelsResponse {
            models: vec![
                sample_model("spur-route-k3deadbeef001", "Kimi · K3"),
                sample_model("spur-route-gpt5luna00001", "OpenAI · GPT-5.6-Luna"),
            ],
        };
        let result = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply");
        assert_eq!(result.model_count, 2);
        assert!(result.config_path.starts_with(&publish_dir));
        assert!(!result.config_path.starts_with(&orca_dir));
        assert!(publish_dir.join("codex-select/model-catalog.json").exists());
        assert!(!orca_dir.join("codex-select/model-catalog.json").exists());

        let catalog_raw = fs::read_to_string(&result.catalog_path).expect("catalog");
        assert!(catalog_raw.contains("\"display_name\""));
        assert!(!catalog_raw.contains("\"displayName\""));
        assert!(catalog_raw.contains("spur-route-k3deadbeef001"));
        assert!(catalog_raw.contains("\"additional_speed_tiers\": []"));
        assert!(catalog_raw.contains("\"service_tiers\": []"));
        assert!(catalog_raw.contains("\"availability_nux\": null"));
        assert!(catalog_raw.contains("\"upgrade\": null"));

        let config_raw = fs::read_to_string(&result.config_path).expect("config");
        assert!(config_raw.contains("model_provider = \"codex_select\""));
        assert!(config_raw.contains(
            "model_catalog_json = \"codex-select/model-catalog.json\""
        ));
        assert!(config_raw.contains("[model_providers.codex_select]"));
        assert!(config_raw.contains("model = \"spur-route-k3deadbeef001\""));
        // Desktop gate: OpenAI name + requires_openai_auth so custom models show in GUI.
        assert!(config_raw.contains("name = \"OpenAI\""));
        assert!(config_raw.contains("requires_openai_auth = true"));
        assert!(config_raw.contains("experimental_bearer_token = \"test-token\""));

        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&publish_dir);
        let _ = fs::remove_dir_all(&orca_dir);
    }

    #[test]
    fn apply_writes_snake_case_catalog_and_codex_select_provider() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-apply-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);

        let catalog = ModelsResponse {
            models: vec![
                sample_model("spur-route-k3deadbeef001", "Kimi · K3"),
                sample_model("spur-route-k2deadbeef002", "Kimi · K2"),
            ],
        };
        let result = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply");
        assert_eq!(result.model_count, 2);
        assert_eq!(
            result.selected_model.as_deref(),
            Some("spur-route-k3deadbeef001")
        );
        assert_eq!(result.model_labels.len(), 2);

        let catalog_raw = fs::read_to_string(&result.catalog_path).expect("catalog");
        assert!(catalog_raw.contains("\"display_name\""));
        assert!(!catalog_raw.contains("\"displayName\""));
        assert!(catalog_raw.contains("\"supported_reasoning_levels\""));
        assert!(catalog_raw.contains("\"additional_speed_tiers\": []"));
        assert!(catalog_raw.contains("\"service_tiers\": []"));
        assert!(catalog_raw.contains("\"availability_nux\": null"));
        assert!(catalog_raw.contains("\"upgrade\": null"));

        let config_raw = fs::read_to_string(&result.config_path).expect("config");
        assert!(config_raw.contains("model_provider = \"codex_select\""));
        assert!(config_raw.contains("[model_providers.codex_select]"));
        assert!(config_raw.contains("model = \"spur-route-k3deadbeef001\""));
        assert!(config_raw.contains("experimental_bearer_token = \"test-token\""));
        assert!(config_raw.contains("name = \"OpenAI\""));
        assert!(config_raw.contains("requires_openai_auth = true"));
        assert!(
            config_raw.contains("supports_websockets = false"),
            "provider must disable websockets for local proxy SSE"
        );
        // History UX: never leave response storage disabled after Spur apply.
        assert!(
            config_raw.contains("disable_response_storage = false")
                || !config_raw.contains("disable_response_storage = true"),
            "apply must not leave disable_response_storage=true"
        );

        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_rejects_slash_slugs() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-slash-slug-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("uuid/k3", "Kimi · K3")],
        };
        let err = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("slug"), "unexpected error: {message}");
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_does_not_treat_config_read_errors_as_empty() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-read-error-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("config.toml")).expect("config directory");
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("spur-route-valid", "Kimi · K3")],
        };

        let error = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).unwrap_err();
        assert!(error.to_string().contains("读取 Codex config.toml 失败"));
        assert!(dir.join("config.toml").is_dir());
        assert!(!dir.join("codex-select/model-catalog.json").exists());

        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_restores_existing_file_and_removes_new_file() {
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-rollback-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("rollback dir");
        let existing = dir.join("existing.toml");
        let created = dir.join("created.json");
        fs::write(&existing, b"changed").expect("changed");
        fs::write(&created, b"new").expect("new");

        rollback_file(&existing, Some(b"original")).expect("restore");
        rollback_file(&created, None).expect("remove");
        assert_eq!(fs::read(&existing).expect("existing"), b"original");
        assert!(!created.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
