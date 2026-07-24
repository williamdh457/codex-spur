use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::domain::{
    ApplyPreview, DesktopVisibility, DesktopVisibilityCheck, ModelsResponse, ReasoningEffort,
};

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
    // Prefer HOME (Unix / Git Bash / some launchers), then USERPROFILE (native Windows).
    // Codex Desktop/CLI config is expected under <user home>/.codex on both platforms.
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".codex");
    }
    if let Some(profile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return PathBuf::from(profile).join(".codex");
    }
    PathBuf::from(".codex")
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

/// Error text when Apply would publish custom models the Desktop GUI will hide.
pub const DESKTOP_AUTH_REQUIRED_MSG: &str = "未检测到有效的 ChatGPT 官方登录（用户 Codex 目录下的 auth.json）。Desktop 会隐藏 Kimi/DeepSeek 等自定义模型，只显示官方 GPT-5.6。请先在 ChatGPT / Codex Desktop 登录官方账号（不是 Codex Spur 的 API Key / 浏览器 OAuth），再完全退出应用后重开，然后重新 Review & Apply。Spur 不会改写 auth.json。";

/// Top-level Codex config key that gates Desktop's hosted web_search tool.
/// Aligns with CC Switch: some native `/responses` gateways hard-400 on that tool.
pub const CODEX_WEB_SEARCH_FIELD: &str = "web_search";
/// Sentinel value we own. Only remove `web_search` when its value equals this string.
pub const CODEX_WEB_SEARCH_DISABLED: &str = "disabled";

/// Model-label / slug brand markers for gateways that reject hosted `web_search`.
/// Blacklist (default-on): unknown brands keep Codex default. Matched against
/// catalog display_name / description / slug (proxy base is always localhost).
const CODEX_WEB_SEARCH_REJECT_MARKERS: &[&str] = &[
    "minimax",
    "mimo",
    "longcat",
    "qwen3-coder",
    "xiaomimimo",
    "minimaxi",
];

/// True when any published catalog row looks like a gateway that rejects web_search.
pub fn catalog_rejects_web_search(catalog: &ModelsResponse) -> bool {
    catalog.models.iter().any(|model| {
        let hay = format!(
            "{} {} {}",
            model.slug,
            model.display_name,
            model.description.as_deref().unwrap_or("")
        )
        .to_ascii_lowercase();
        CODEX_WEB_SEARCH_REJECT_MARKERS
            .iter()
            .any(|marker| hay.contains(marker))
    })
}

/// Set or clear the Spur-owned `web_search = "disabled"` sentinel on a TOML document.
fn apply_web_search_sentinel(document: &mut DocumentMut, disable: bool) {
    if disable {
        document[CODEX_WEB_SEARCH_FIELD] = value(CODEX_WEB_SEARCH_DISABLED);
        return;
    }
    let is_our_sentinel = document
        .get(CODEX_WEB_SEARCH_FIELD)
        .and_then(|item| item.as_str())
        == Some(CODEX_WEB_SEARCH_DISABLED);
    if is_our_sentinel {
        document.as_table_mut().remove(CODEX_WEB_SEARCH_FIELD);
    }
}

/// True when any catalog row is a non-native Desktop slug (Kimi/DeepSeek/spur-route-…).
/// Official-only gpt-5.6-* catalogs may still publish without ChatGPT login.
pub fn catalog_requires_chatgpt_auth(catalog: &ModelsResponse) -> bool {
    catalog
        .models
        .iter()
        .any(|model| !crate::providers::is_desktop_native_model_slug(&model.slug))
}

pub fn inspect_live_binding() -> LiveBinding {
    inspect_live_binding_with_proxy(None, None)
}

pub fn inspect_live_binding_with_proxy(
    proxy_running: Option<bool>,
    proxy_base_url: Option<&str>,
) -> LiveBinding {
    let visibility = inspect_desktop_visibility(proxy_running, proxy_base_url);
    let home = PathBuf::from(&visibility.codex_home);
    let catalog_path = catalog_path_for(&home);
    let state = binding_state_from_visibility(&visibility);
    let attention = attention_from_desktop_visibility(&visibility);

    // CODEX_HOME isolation (Orca/Herdr) is informational only when we already publish
    // to the real ~/.codex path ChatGPT reads.
    let _ = env_codex_home_diverges(&home);

    LiveBinding {
        state,
        codex_home: home,
        provider_id: "codex_select".into(),
        catalog_path,
        attention,
    }
}

/// Structured Desktop model-picker readiness (Nice Switch “preserve official login” gate).
///
/// `proxy_running` / `proxy_base_url` come from the Spur runtime; pass `None` when unknown.
pub fn inspect_desktop_visibility(
    proxy_running: Option<bool>,
    proxy_base_url: Option<&str>,
) -> DesktopVisibility {
    let home = publish_codex_home();
    let catalog_path = catalog_path_for(&home);
    let config_path = config_path_for(&home);
    let mut checks = Vec::new();

    // 1) ChatGPT Desktop identity (auth.json) — required for custom catalog rows in GUI.
    let auth_status = chatgpt_auth_status(&home);
    let auth_ok = auth_status == AuthStatus::Ok;
    checks.push(DesktopVisibilityCheck {
        id: "chatgpt_auth".into(),
        label: "ChatGPT 官方登录".into(),
        ok: auth_ok,
        detail: match auth_status {
            AuthStatus::Ok => "~/.codex/auth.json 有效（Desktop 身份门控）".into(),
            AuthStatus::Missing => {
                "缺少 auth.json。请在 ChatGPT.app 登录官方账号，不是 Spur 的 API Key / OAuth".into()
            }
            AuthStatus::Unreadable => "auth.json 无法解析或缺少 tokens".into(),
        },
    });

    // 2–3) Parse config for provider gate + binding.
    let config_raw = fs::read_to_string(&config_path).ok();
    let document = config_raw
        .as_deref()
        .and_then(|raw| raw.parse::<DocumentMut>().ok());

    let provider = document
        .as_ref()
        .and_then(|doc| doc.get("model_provider"))
        .and_then(|item| item.as_str())
        .unwrap_or("");
    let catalog_ref = document
        .as_ref()
        .and_then(|doc| doc.get("model_catalog_json"))
        .and_then(|item| item.as_str())
        .unwrap_or("");

    let select_table = document
        .as_ref()
        .and_then(|doc| doc.get("model_providers"))
        .and_then(|item| item.get("codex_select"));
    let gate_name_ok = select_table
        .and_then(|item| item.get("name"))
        .and_then(|item| item.as_str())
        == Some("OpenAI");
    let gate_auth_ok = select_table
        .and_then(|item| item.get("requires_openai_auth"))
        .and_then(|item| item.as_bool())
        == Some(true);
    let catalog_path_ok = {
        let expected = catalog_path.display().to_string();
        catalog_ref == expected
            || (Path::new(catalog_ref)
                .file_name()
                .is_some_and(|name| name == "model-catalog.json")
                && catalog_ref.contains("codex-select"))
    };
    let applied = provider == "codex_select" && catalog_path_ok && catalog_path.exists();
    let provider_gate_ok = applied && gate_name_ok && gate_auth_ok;

    let gate_detail = if !applied {
        if provider == "custom" || catalog_ref.contains("cc-switch") {
            "仍绑定 CC Switch（custom）；请 Review & Apply 到 codex_select".into()
        } else if provider.is_empty() {
            "尚未应用 codex_select provider".into()
        } else {
            format!("当前 model_provider = \"{provider}\"，尚未切换到 codex_select")
        }
    } else if !gate_name_ok || !gate_auth_ok {
        "门控被改坏：需要 name = \"OpenAI\" 且 requires_openai_auth = true；请重新 Apply".into()
    } else {
        "name = OpenAI + requires_openai_auth = true".into()
    };
    checks.push(DesktopVisibilityCheck {
        id: "provider_gate".into(),
        label: "Provider 门控".into(),
        ok: provider_gate_ok,
        detail: gate_detail,
    });

    // 4) Catalog on disk shape.
    let (catalog_ok, catalog_detail) = match fs::read_to_string(&catalog_path) {
        Err(_) if !applied => (false, "尚未写入 model-catalog.json".into()),
        Err(_) => (false, format!("无法读取 {}", catalog_path.display())),
        Ok(raw) => {
            if raw.contains("\"displayName\"")
                || raw.contains("\"supportedReasoningLevels\"")
                || raw.contains("\"experimentalSupportedTools\"")
            {
                (
                    false,
                    "catalog 含 camelCase 字段，Desktop 会 Invalid configuration".into(),
                )
            } else if !raw.contains("\"experimental_supported_tools\"") {
                (
                    false,
                    "catalog 缺少 experimental_supported_tools（Codex 硬依赖）".into(),
                )
            } else {
                match serde_json::from_str::<ModelsResponse>(&raw) {
                    Ok(parsed) => match crate::catalog::validate_catalog(&parsed) {
                        Ok(()) => (true, format!("{} 个模型，形状合法", parsed.models.len())),
                        Err(error) => (false, format!("catalog 校验失败：{error}")),
                    },
                    Err(error) => (false, format!("catalog JSON 无法解析：{error}")),
                }
            }
        }
    };
    checks.push(DesktopVisibilityCheck {
        id: "catalog".into(),
        label: "Catalog 形状".into(),
        ok: catalog_ok,
        detail: catalog_detail,
    });

    // 5) Local proxy.
    let config_base = select_table
        .and_then(|item| item.get("base_url"))
        .and_then(|item| item.as_str());
    let proxy_ok = match proxy_running {
        Some(true) => {
            if let (Some(expected), Some(actual)) = (config_base, proxy_base_url) {
                expected == actual || actual.starts_with("http://127.0.0.1:")
            } else {
                true
            }
        }
        Some(false) => false,
        None => config_base.is_some_and(|url| url.contains("127.0.0.1")),
    };
    let proxy_detail = match (proxy_running, config_base, proxy_base_url) {
        (Some(false), _, _) => "本地代理未运行".into(),
        (Some(true), Some(expected), Some(actual)) if expected != actual => {
            format!("代理 {actual} 与 config base_url {expected} 不一致")
        }
        (Some(true), _, Some(actual)) => format!("代理运行中 {actual}"),
        (_, Some(url), _) => format!("config base_url = {url}"),
        _ => "尚未配置本地代理 base_url".into(),
    };
    checks.push(DesktopVisibilityCheck {
        id: "proxy".into(),
        label: "本地代理".into(),
        ok: proxy_ok,
        detail: proxy_detail,
    });

    // 6) ChatGPT process (informational for cold start; does not alone define ready).
    let chatgpt_running = chatgpt_desktop_running();
    checks.push(DesktopVisibilityCheck {
        id: "chatgpt_process".into(),
        label: "冷启动状态".into(),
        ok: !chatgpt_running,
        detail: if chatgpt_running {
            "ChatGPT 仍在运行：catalog 仅冷启动加载，改配置后需完全退出应用再开".into()
        } else {
            "ChatGPT 未在运行；下次打开将加载当前 catalog".into()
        },
    });

    // 7) CC Switch conflict.
    let hooks = hooks_mention_cc_switch(&home);
    let bound_cc = provider == "custom" || catalog_ref.contains("cc-switch");
    let cc_ok = !bound_cc;
    let cc_detail = if bound_cc {
        "Codex 仍绑定 CC Switch；请 Review & Apply".into()
    } else if hooks {
        "SessionStart 仍有 CC Switch 同步脚本；勿再将其设为 current".into()
    } else {
        "未检测到 CC Switch 抢占".into()
    };
    checks.push(DesktopVisibilityCheck {
        id: "cc_switch".into(),
        label: "CC Switch 冲突".into(),
        ok: cc_ok && !hooks,
        detail: cc_detail,
    });
    // Hooks alone: soft — ok flag already false when hooks; keep ready tolerant if still applied.
    let cc_blocks_ready = bound_cc;

    let ready = auth_ok && applied && provider_gate_ok && catalog_ok && !cc_blocks_ready;
    let status_label = if ready {
        "就绪".into()
    } else if !auth_ok {
        "缺登录".into()
    } else if !applied {
        "待应用".into()
    } else {
        "异常".into()
    };

    DesktopVisibility {
        ready,
        status_label,
        codex_home: home.display().to_string(),
        checks,
    }
}

fn binding_state_from_visibility(visibility: &DesktopVisibility) -> String {
    let home = PathBuf::from(&visibility.codex_home);
    let config_path = config_path_for(&home);
    let catalog_path = catalog_path_for(&home);
    let Ok(raw) = fs::read_to_string(config_path) else {
        return "not_applied".into();
    };
    let Ok(document) = raw.parse::<DocumentMut>() else {
        return "invalid".into();
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
            || (Path::new(catalog_ref)
                .file_name()
                .is_some_and(|name| name == "model-catalog.json")
                && catalog_ref.contains("codex-select"));
        if catalog_ok && catalog_path.exists() {
            // Applied even if gate fields were hand-edited wrong (visibility shows 异常).
            return "applied".into();
        }
        return "invalid".into();
    }
    "not_applied".into()
}

/// Free-text attention lines for Overview, derived from structured readiness.
pub fn attention_from_desktop_visibility(visibility: &DesktopVisibility) -> Vec<String> {
    let mut attention = Vec::new();
    for check in &visibility.checks {
        if check.ok {
            continue;
        }
        // Cold-start is shown in the checklist only; Apply path already toasts Cmd+Q
        // when ChatGPT is still running. Do not spam 需要处理 while the user is in Codex.
        if check.id == "chatgpt_process" {
            continue;
        }
        match check.id.as_str() {
            "chatgpt_auth" => attention.push(DESKTOP_AUTH_REQUIRED_MSG.into()),
            "provider_gate" | "catalog" | "proxy" | "cc_switch" => {
                attention.push(format!("{}：{}", check.label, check.detail));
            }
            _ => attention.push(check.detail.clone()),
        }
    }
    attention
}

fn env_codex_home_diverges(publish_home: &Path) -> bool {
    env::var_os("CODEX_HOME").is_some_and(|value| {
        let env_home = PathBuf::from(value);
        env_home != publish_home
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthStatus {
    Ok,
    Missing,
    Unreadable,
}

/// True when ~/.codex/auth.json looks like a ChatGPT/Codex OAuth login (not API-key only).
/// Used for Desktop custom-model visibility diagnostics — never reads token bodies into logs.
fn chatgpt_auth_present(home: &Path) -> bool {
    chatgpt_auth_status(home) == AuthStatus::Ok
}

fn chatgpt_auth_status(home: &Path) -> AuthStatus {
    let path = home.join("auth.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return AuthStatus::Missing;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return AuthStatus::Unreadable;
    };
    let mode = value
        .get("auth_mode")
        .and_then(|item| item.as_str())
        .unwrap_or("");
    if mode == "chatgpt" || mode == "codex" {
        return if value.get("tokens").is_some() {
            AuthStatus::Ok
        } else {
            AuthStatus::Unreadable
        };
    }
    // Legacy shapes: tokens object with refresh/access is enough for Desktop gating.
    let ok = value
        .get("tokens")
        .and_then(|tokens| tokens.as_object())
        .is_some_and(|tokens| {
            tokens.contains_key("access_token") || tokens.contains_key("refresh_token")
        });
    if ok {
        AuthStatus::Ok
    } else if value.get("tokens").is_some() {
        AuthStatus::Unreadable
    } else {
        AuthStatus::Missing
    }
}

fn hooks_mention_cc_switch(home: &Path) -> bool {
    let hooks = home.join("hooks.json");
    fs::read_to_string(hooks)
        .map(|text| text.contains("cc-switch-routing") || text.contains("session_start_sync"))
        .unwrap_or(false)
}

pub fn preview(base_url: &str, catalog: &ModelsResponse) -> ApplyPreview {
    let home = publish_codex_home();
    let catalog_path = catalog_path_for(&home);
    let config_path = config_path_for(&home);
    let model_count = catalog.models.len() as u32;
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
    if catalog_requires_chatgpt_auth(catalog) && !chatgpt_auth_present(&home) {
        warnings.push(DESKTOP_AUTH_REQUIRED_MSG.into());
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
    for item in live.attention {
        if !warnings.iter().any(|existing| existing == &item) {
            warnings.push(item);
        }
    }
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
    // Hard-block: custom catalog rows need ChatGPT Desktop identity or GUI hides them.
    if catalog_requires_chatgpt_auth(catalog) && !chatgpt_auth_present(&home) {
        anyhow::bail!("{DESKTOP_AUTH_REQUIRED_MSG}");
    }
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
        atomic_write(
            &path,
            original_config.as_deref().unwrap_or_default().as_bytes(),
        )?;
        Some(path)
    } else {
        None
    };

    let mut document = if original_config
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
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

    // CC Switch pattern: disable Desktop hosted web_search when any published
    // model targets a gateway known to 400 on that tool. Only clear our own sentinel.
    apply_web_search_sentinel(&mut document, catalog_rejects_web_search(catalog));

    let config_bytes = document.to_string().into_bytes();
    let write_result = (|| -> Result<()> {
        atomic_write(&catalog_path, &catalog_json)?;
        atomic_write(&config_path, &config_bytes)?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let rollback_result = rollback_file(&catalog_path, original_catalog.as_deref()).and(
            rollback_file(&config_path, original_config.as_ref().map(String::as_bytes)),
        );
        return match rollback_result {
            Ok(()) => Err(error).context("Codex catalog/config 写入失败，已回滚"),
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
                Err(rollback_error) => {
                    Err(error).context(format!("应用后校验失败，且回滚失败：{rollback_error}"))
                }
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
            "ChatGPT 仍在运行：模型目录只在启动时加载。请现在完全退出 ChatGPT/Codex，再重新打开，否则菜单不会出现新写入的 Kimi 等模型。"
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
                || model.description.as_ref().is_some_and(|text| {
                    text.to_ascii_lowercase()
                        .contains(&bare.to_ascii_lowercase())
                })
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
    file.sync_all()
        .with_context(|| format!("刷新临时文件失败：{}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("原子替换文件失败：{}", path.display()))?;
    if let Some(parent) = path.parent() {
        let directory = fs::File::open(parent)?;
        directory
            .sync_all()
            .with_context(|| format!("刷新目录失败：{}", parent.display()))?;
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
    #[test]
    fn user_codex_home_prefers_home_then_userprofile() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let prev_home = env::var_os("HOME");
        let prev_profile = env::var_os("USERPROFILE");
        let prev_codex_home = env::var_os("CODEX_HOME");
        let prev_publish = env::var_os("CODEX_SPUR_PUBLISH_HOME");
        env::remove_var("CODEX_HOME");
        env::remove_var("CODEX_SPUR_PUBLISH_HOME");

        env::set_var("HOME", "/tmp/spur-home-a");
        env::set_var("USERPROFILE", "C:\\Users\\spur");
        assert_eq!(user_codex_home(), PathBuf::from("/tmp/spur-home-a/.codex"));

        env::remove_var("HOME");
        assert_eq!(
            user_codex_home(),
            PathBuf::from(r"C:\Users\spur").join(".codex")
        );

        env::remove_var("USERPROFILE");
        assert_eq!(user_codex_home(), PathBuf::from(".codex"));

        match prev_home {
            Some(value) => env::set_var("HOME", value),
            None => env::remove_var("HOME"),
        }
        match prev_profile {
            Some(value) => env::set_var("USERPROFILE", value),
            None => env::remove_var("USERPROFILE"),
        }
        match prev_codex_home {
            Some(value) => env::set_var("CODEX_HOME", value),
            None => env::remove_var("CODEX_HOME"),
        }
        match prev_publish {
            Some(value) => env::set_var("CODEX_SPUR_PUBLISH_HOME", value),
            None => env::remove_var("CODEX_SPUR_PUBLISH_HOME"),
        }
    }

    /// Minimal valid ChatGPT Desktop identity for apply hard-block tests (no real secrets).
    fn write_test_chatgpt_auth(home: &Path) {
        fs::write(
            home.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test-access","refresh_token":"test-refresh"}}"#,
        )
        .expect("write test auth.json");
    }

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
            effective_context_window_percent:
                crate::domain::DEFAULT_EFFECTIVE_CONTEXT_WINDOW_PERCENT,
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
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let publish_dir = std::env::temp_dir().join(format!(
            "codex-spur-publish-{}-{}",
            std::process::id(),
            stamp
        ));
        let orca_dir =
            std::env::temp_dir().join(format!("codex-spur-orca-{}-{}", std::process::id(), stamp));
        fs::create_dir_all(&publish_dir).expect("publish dir");
        fs::create_dir_all(&orca_dir).expect("orca dir");
        write_test_chatgpt_auth(&publish_dir);
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
        assert!(config_raw.contains("model_catalog_json = \"codex-select/model-catalog.json\""));
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
    fn catalog_rejects_web_search_matches_minimax_labels() {
        assert!(catalog_rejects_web_search(&ModelsResponse {
            models: vec![sample_model("spur-route-mm", "MiniMax · abab6.5")],
        }));
        assert!(!catalog_rejects_web_search(&ModelsResponse {
            models: vec![sample_model("spur-route-k3", "Kimi · K3")],
        }));
    }

    #[test]
    fn apply_writes_web_search_disabled_for_minimax_catalog() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-websearch-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        write_test_chatgpt_auth(&dir);
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("spur-route-minimax001", "MiniMax · M2")],
        };
        apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply");
        let config_raw = fs::read_to_string(dir.join("config.toml")).expect("config");
        assert!(
            config_raw.contains("web_search = \"disabled\""),
            "expected web_search sentinel, got:\n{config_raw}"
        );
        // Re-apply with only Kimi: clear our sentinel.
        let catalog = ModelsResponse {
            models: vec![sample_model("spur-route-kimi001", "Kimi · K3")],
        };
        apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply2");
        let config_raw = fs::read_to_string(dir.join("config.toml")).expect("config2");
        assert!(
            !config_raw.contains("web_search = \"disabled\""),
            "sentinel should clear when no reject models remain"
        );
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_writes_snake_case_catalog_and_codex_select_provider() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-apply-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        write_test_chatgpt_auth(&dir);
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
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-read-error-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("config.toml")).expect("config directory");
        write_test_chatgpt_auth(&dir);
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
    fn apply_blocks_custom_catalog_without_chatgpt_auth() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-auth-block-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        // Intentionally no auth.json
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("spur-route-kimi001", "Kimi · K2.7")],
        };
        let error = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("auth.json") || message.contains("官方登录"),
            "unexpected: {message}"
        );
        assert!(!dir.join("codex-select/model-catalog.json").exists());
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_allows_official_only_catalog_without_auth() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-official-only-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("gpt-5.6-terra", "GPT-5.6-Terra")],
        };
        let result = apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply");
        assert_eq!(result.model_count, 1);
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_desktop_visibility_reports_missing_auth() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-vis-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let visibility = inspect_desktop_visibility(Some(true), Some("http://127.0.0.1:17861/v1"));
        assert!(!visibility.ready);
        assert_eq!(visibility.status_label, "缺登录");
        assert!(visibility
            .checks
            .iter()
            .any(|check| check.id == "chatgpt_auth" && !check.ok));
        std::env::remove_var("CODEX_SPUR_PUBLISH_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_desktop_visibility_ready_after_apply_with_auth() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "codex-spur-vis-ready-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp home");
        write_test_chatgpt_auth(&dir);
        std::env::set_var("CODEX_SPUR_PUBLISH_HOME", &dir);
        let catalog = ModelsResponse {
            models: vec![sample_model("spur-route-kimi001", "Kimi · K2.7")],
        };
        apply("http://127.0.0.1:17861/v1", "test-token", &catalog).expect("apply");
        let visibility = inspect_desktop_visibility(Some(true), Some("http://127.0.0.1:17861/v1"));
        assert!(visibility.ready, "{visibility:?}");
        assert_eq!(visibility.status_label, "就绪");
        assert!(visibility
            .checks
            .iter()
            .find(|check| check.id == "provider_gate")
            .is_some_and(|check| check.ok));
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
