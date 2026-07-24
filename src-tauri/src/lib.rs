#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod catalog;
pub mod codex_config;
mod content_encoding;
mod credentials;
mod domain;
mod kimi_list_shield;
mod kimi_target;
mod media_sanitizer;
mod openai_agent_identity;
mod openai_oauth;
mod opencode_go;
pub mod providers;
mod proxy;
mod quota;
mod scheduler;
pub mod storage;
mod upstream_errors;
pub mod vault;
mod xai_oauth;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use credentials::{CredentialImportSummary, SecretMaterial};
use domain::{
    AccountPoolSummary, AppSnapshot, ApplyPreview, CodexApplyOutcome, CodexBindingStatus,
    CredentialSummary, DeleteCredentialResult, ModelRouteSummary, OpenAiQuotaSnapshot,
    OpenCodeGoCredentialStatus, PoolMemberDetail, ProviderRouting, ProviderSummary,
    ProxyRequestEvent, ProxyStatus,
};
use scheduler::PoolSchedulerConfig;
use tauri::{
    menu::{MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent,
};
use tokio::sync::{oneshot, Mutex, RwLock};
use uuid::Uuid;
use zeroize::Zeroizing;

pub struct AppState {
    snapshot: RwLock<AppSnapshot>,
    pub(crate) catalog: catalog::SharedCatalog,
    routes: catalog::SharedRoutes,
    storage: Arc<storage::Storage>,
    proxy: RwLock<proxy::ProxyRuntime>,
    vault: Arc<vault::SecretVault>,
    openai_oauth: openai_oauth::OpenAiOAuthManager,
    xai_oauth: xai_oauth::XaiOAuthManager,
    /// Shutdown sender for the active browser OAuth callback listener.
    openai_oauth_listener: Mutex<Option<oneshot::Sender<()>>>,
    /// Optional local CONNECT proxy that blocks www.kimi.com model-list host.
    kimi_list_shield: kimi_list_shield::KimiListShield,
}

impl AppState {
    async fn bootstrap(data_dir: std::path::PathBuf) -> anyhow::Result<Self> {
        let storage = Arc::new(storage::Storage::open(&data_dir).await?);
        let vault = Arc::new(vault::SecretVault::load_or_create(&data_dir)?);
        // Scrub legacy camelCase / GPT-tool ads from SQLite so every subsequent
        // apply/rebuild starts from Codex-safe snake_case rows.
        match storage.heal_all_route_catalogs().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                healed_routes = n,
                "已将 model_routes.catalog_json heal 为 snake_case"
            ),
            Err(error) => {
                tracing::warn!(%error, "启动时 heal catalog_json 失败，将继续用运行时 heal")
            }
        }
        let stored_routes = storage.list_routes(true).await?;
        let (catalog_value, route_values) = catalog::build_from_routes(&stored_routes)?;
        let catalog = Arc::new(RwLock::new(catalog_value));
        let routes = Arc::new(RwLock::new(route_values));
        let proxy_secret = proxy::load_or_create_secret(&data_dir)?;
        let proxy = proxy::start_with_secret(
            Arc::clone(&catalog),
            Arc::clone(&routes),
            Arc::clone(&storage),
            Arc::clone(&vault),
            17_861,
            proxy_secret,
        )
        .await?;
        let base_url = format!("http://127.0.0.1:{}/v1", proxy.port);
        let providers = storage.list_providers().await?;
        let credentials = storage.list_credentials(None).await?;
        let published_models = catalog.read().await.models.len() as u32;
        let desktop_visibility =
            codex_config::inspect_desktop_visibility(Some(true), Some(base_url.as_str()));
        let live =
            codex_config::inspect_live_binding_with_proxy(Some(true), Some(base_url.as_str()));
        let mut attention_items = live.attention;
        if published_models == 0 {
            attention_items.push("添加供应商并拉取模型后，才能应用到 Codex。".into());
        }
        let snapshot = AppSnapshot {
            proxy: ProxyStatus {
                running: true,
                base_url: Some(base_url),
                port: Some(proxy.port),
                catalog_revision: format!("models-{published_models}"),
                last_error: None,
            },
            binding: CodexBindingStatus {
                state: live.state,
                codex_home: live.codex_home.display().to_string(),
                provider_id: live.provider_id,
                catalog_path: live.catalog_path.display().to_string(),
            },
            providers,
            published_models,
            healthy_accounts: credentials.iter().filter(|item| item.healthy).count() as u32,
            attention_items,
            desktop_visibility,
        };
        let state = Self {
            snapshot: RwLock::new(snapshot),
            catalog,
            routes,
            storage,
            proxy: RwLock::new(proxy),
            vault,
            openai_oauth: openai_oauth::OpenAiOAuthManager::new(),
            xai_oauth: xai_oauth::XaiOAuthManager::new(),
            openai_oauth_listener: Mutex::new(None),
            kimi_list_shield: kimi_list_shield::KimiListShield::new(),
        };
        // Older builds left system proxy → 127.0.0.1:17862 which breaks Kimi entirely.
        clear_residual_kimi_system_proxy_if_needed();
        Ok(state)
    }

    async fn rebuild_runtime(&self) -> Result<(), String> {
        let stored_routes = self
            .storage
            .list_routes(true)
            .await
            .map_err(|error| error.to_string())?;
        let (catalog_value, route_values) =
            catalog::build_from_routes(&stored_routes).map_err(|error| error.to_string())?;
        let published_models = catalog_value.models.len() as u32;
        let providers = self
            .storage
            .list_providers()
            .await
            .map_err(|error| error.to_string())?;

        // Snapshot proxy fields without holding write lock across disk/Codex inspect.
        let (proxy_running, proxy_base) = {
            let snapshot = self.snapshot.read().await;
            (snapshot.proxy.running, snapshot.proxy.base_url.clone())
        };
        let desktop_visibility =
            codex_config::inspect_desktop_visibility(Some(proxy_running), proxy_base.as_deref());
        let live = codex_config::inspect_live_binding_with_proxy(
            Some(proxy_running),
            proxy_base.as_deref(),
        );
        let mut attention = live.attention;
        if published_models == 0 {
            attention.push("添加供应商并拉取模型后，才能应用到 Codex。".into());
        }

        *self.catalog.write().await = catalog_value;
        *self.routes.write().await = route_values;
        {
            let mut snapshot = self.snapshot.write().await;
            snapshot.published_models = published_models;
            snapshot.proxy.catalog_revision = format!("models-{published_models}");
            snapshot.providers = providers;
            snapshot.binding.state = live.state;
            snapshot.binding.codex_home = live.codex_home.display().to_string();
            snapshot.binding.provider_id = live.provider_id;
            snapshot.binding.catalog_path = live.catalog_path.display().to_string();
            snapshot.desktop_visibility = desktop_visibility;
            snapshot.attention_items = attention;
        }
        Ok(())
    }

    async fn restart_proxy(&self) -> Result<(), String> {
        let preferred_port = self.proxy.read().await.port;
        self.proxy.read().await.stop().await;
        let runtime = proxy::start(
            Arc::clone(&self.catalog),
            Arc::clone(&self.routes),
            Arc::clone(&self.storage),
            Arc::clone(&self.vault),
            preferred_port,
        )
        .await
        .map_err(|error| error.to_string())?;
        let runtime_port = runtime.port;
        let base_url = format!("http://127.0.0.1:{runtime_port}/v1");
        {
            let mut proxy = self.proxy.write().await;
            *proxy = runtime;
        }
        let mut snapshot = self.snapshot.write().await;
        snapshot.proxy.running = true;
        snapshot.proxy.port = Some(runtime_port);
        snapshot.proxy.base_url = Some(base_url);
        snapshot.proxy.last_error = None;
        Ok(())
    }

    async fn shutdown(&self) {
        self.proxy.read().await.stop().await;
        if let Err(error) = self.storage.release_all_leases().await {
            tracing::warn!(%error, "failed to release account leases during shutdown");
        }
    }
}

/// Encrypt and persist a full secret envelope (tokens + agent identity fields).
async fn persist_credential_secret(
    state: &AppState,
    credential_id: &str,
    secret: &SecretMaterial,
    expires_at: Option<i64>,
    account_id: Option<&str>,
) -> Result<(), String> {
    let json = providers::secret_material_json(secret).map_err(|error| error.to_string())?;
    let envelope = state
        .vault
        .encrypt(credential_id, 1, json.as_slice())
        .map_err(|error| error.to_string())?;
    let envelope_json = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
    state
        .storage
        .update_credential_secret(credential_id, &envelope_json, expires_at, account_id)
        .await
        .map_err(|error| error.to_string())
}

async fn quota_access(state: &AppState, credential_id: &str) -> Result<(String, String), String> {
    let credential = state
        .storage
        .get_credential(credential_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    // provider_id is the instance UUID, not the kind string "openai".
    let provider = state
        .storage
        .get_provider(&credential.provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号所属供应商实例不存在".to_string())?;
    if provider.kind != "openai" {
        return Err("额度接口仅适用于 OpenAI 官方订阅账号".into());
    }
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| error.to_string())?;
    let mut secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;

    let mut account_id = credential
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            secret
                .access_token
                .as_deref()
                .and_then(openai_oauth::chatgpt_account_id_from_token)
        })
        .or_else(|| {
            secret
                .id_token
                .as_deref()
                .and_then(openai_oauth::chatgpt_account_id_from_token)
        });

    // Refresh when access token is near expiry — or when account_id is still
    // missing (common for partial JSON imports). Sub2API recovers account_id
    // from a fresh OAuth response; we do the same without copying its code.
    let needs_refresh = secret
        .access_token
        .as_deref()
        .map(|access| {
            openai_oauth::access_token_needs_refresh(access, None) || account_id.is_none()
        })
        .unwrap_or(account_id.is_none());

    if needs_refresh {
        if let Some(refresh) = secret
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            match openai_oauth::refresh_chatgpt_tokens(refresh, secret.id_token.as_deref()).await {
                Ok(refreshed) => {
                    secret.access_token = Some(refreshed.access_token.clone());
                    if let Some(id_token) = refreshed.id_token {
                        secret.id_token = Some(id_token);
                    }
                    if let Some(new_refresh) = refreshed.refresh_token {
                        secret.refresh_token = Some(new_refresh);
                    }
                    if !refreshed.account_id.trim().is_empty() {
                        account_id = Some(refreshed.account_id.clone());
                    } else {
                        account_id = account_id.or_else(|| {
                            openai_oauth::chatgpt_account_id_from_token(&refreshed.access_token)
                        });
                    }
                    // Full secret writeback — never drop agent_* fields.
                    let _ = persist_credential_secret(
                        state,
                        &credential.id,
                        &secret,
                        refreshed.expires_at,
                        account_id.as_deref(),
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(
                        credential_id = %credential.id,
                        "quota token refresh failed; trying existing access token: {error}"
                    );
                }
            }
        }
    }

    let access_token = secret
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            if secret.is_agent_identity() {
                "此账号为 Agent Identity，本地未保留 access_token，无法查官方 5h/7d。请重新导入带 accessToken 的 session / 账号 JSON（将自动合并到本账号）。代理调用不受影响。".to_string()
            } else {
                "账号没有 access_token，无法查询官方订阅额度".to_string()
            }
        })?;
    // JWT first, then network recover (accounts/check / whoami) — Sub2API-style
    // so partial JSON imports can still pull 5h/7d usage without re-login.
    let account_id = openai_oauth::ensure_chatgpt_account_id(
        &access_token,
        secret.id_token.as_deref(),
        account_id.as_deref(),
    )
    .await
    .map_err(|error| {
        format!(
            "账号缺少 ChatGPT account_id（JSON 未写入且 refresh/token 无法解析，网络补拉也失败）：{error}。可重新导入带 account_id 的账号 JSON，或重新官方登录。"
        )
    })?;

    // Persist recovered account_id so later refreshes and pool routing see it.
    if credential.account_id.as_deref() != Some(account_id.as_str()) {
        let _ = persist_credential_secret(
            state,
            &credential.id,
            &secret,
            None,
            Some(account_id.as_str()),
        )
        .await;
    }
    Ok((access_token, account_id))
}

async fn refresh_quota_for_state(
    state: &AppState,
    credential_id: &str,
) -> Result<OpenAiQuotaSnapshot, String> {
    let (access_token, account_id) = quota_access(state, credential_id).await?;
    let snapshot = quota::fetch(credential_id, &access_token, &account_id)
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .save_quota_snapshot(&snapshot)
        .await
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
}

#[tauri::command]
async fn get_usage_snapshot(state: State<'_, AppState>) -> Result<domain::UsageSnapshot, String> {
    state
        .storage
        .usage_snapshot()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_usage_dashboard(
    range: domain::UsageRange,
    state: State<'_, AppState>,
) -> Result<domain::UsageDashboardSnapshot, String> {
    state
        .storage
        .usage_dashboard(range)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_app_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    let providers = state
        .storage
        .list_providers()
        .await
        .map_err(|error| error.to_string())?;
    let credentials = state
        .storage
        .list_credentials(None)
        .await
        .map_err(|error| error.to_string())?;
    let published_models = state.catalog.read().await.models.len() as u32;
    let mut snapshot = state.snapshot.write().await;
    snapshot.providers = providers;
    snapshot.healthy_accounts = credentials.iter().filter(|item| item.healthy).count() as u32;
    // Drift re-check: auth expiry, gate edits, CC Switch steal-back.
    let proxy_running = snapshot.proxy.running;
    let proxy_base = snapshot.proxy.base_url.clone();
    let desktop_visibility =
        codex_config::inspect_desktop_visibility(Some(proxy_running), proxy_base.as_deref());
    let live =
        codex_config::inspect_live_binding_with_proxy(Some(proxy_running), proxy_base.as_deref());
    snapshot.binding.state = live.state;
    snapshot.binding.codex_home = live.codex_home.display().to_string();
    snapshot.binding.provider_id = live.provider_id;
    snapshot.binding.catalog_path = live.catalog_path.display().to_string();
    snapshot.desktop_visibility = desktop_visibility;
    snapshot.published_models = published_models;
    snapshot.attention_items = live.attention;
    if published_models == 0 {
        snapshot
            .attention_items
            .push("添加供应商并拉取模型后，才能应用到 Codex。".into());
    }
    Ok(snapshot.clone())
}

#[tauri::command]
async fn preview_codex_apply(state: State<'_, AppState>) -> Result<ApplyPreview, String> {
    let snapshot = state.snapshot.read().await;
    let catalog = state.catalog.read().await.clone();
    let base_url = snapshot
        .proxy
        .base_url
        .clone()
        .ok_or_else(|| "本地代理尚未启动".to_string())?;
    Ok(codex_config::preview(&base_url, &catalog))
}

#[tauri::command]
async fn apply_codex_config(state: State<'_, AppState>) -> Result<CodexApplyOutcome, String> {
    let snapshot = state.snapshot.read().await;
    let base_url = snapshot
        .proxy
        .base_url
        .clone()
        .ok_or_else(|| "本地代理尚未启动".to_string())?;
    drop(snapshot);
    // Rebuild catalog from DB so publish always heals stale route JSON.
    state.rebuild_runtime().await?;
    let catalog = state.catalog.read().await.clone();
    let proxy = state.proxy.read().await;
    let result = codex_config::apply(&base_url, &proxy.secret, &catalog)
        .map_err(|error| error.to_string())?;
    // Fail closed: re-read live publish home; never toast success if still CC Switch.
    let live = codex_config::inspect_live_binding();
    if live.state != "applied" {
        return Err(format!(
            "写入后校验失败：{} 仍不是 codex_select（state={}）。路径：{}",
            live.codex_home.display(),
            live.state,
            result.config_path.display()
        ));
    }
    let revision_id = Uuid::new_v4().to_string();
    let _ = state
        .storage
        .record_apply_revision(
            &revision_id,
            &result.catalog_path.display().to_string(),
            &result.config_path.display().to_string(),
            result.before_hash.as_deref(),
            &result.after_hash,
            "applied",
        )
        .await;
    {
        let mut snapshot = state.snapshot.write().await;
        let proxy_running = snapshot.proxy.running;
        let proxy_base = snapshot.proxy.base_url.clone();
        let desktop_visibility =
            codex_config::inspect_desktop_visibility(Some(proxy_running), proxy_base.as_deref());
        let live_proxy = codex_config::inspect_live_binding_with_proxy(
            Some(proxy_running),
            proxy_base.as_deref(),
        );
        snapshot.binding.state = live_proxy.state;
        snapshot.binding.codex_home = live_proxy.codex_home.display().to_string();
        snapshot.binding.provider_id = live_proxy.provider_id.clone();
        snapshot.binding.catalog_path = live_proxy.catalog_path.display().to_string();
        snapshot.published_models = result.model_count;
        snapshot.desktop_visibility = desktop_visibility;
        snapshot.attention_items = live_proxy.attention;
        for warning in &result.warnings {
            if !snapshot.attention_items.iter().any(|item| item == warning) {
                snapshot.attention_items.push(warning.clone());
            }
        }
    }
    Ok(CodexApplyOutcome {
        config_path: result.config_path.display().to_string(),
        catalog_path: result.catalog_path.display().to_string(),
        backup_path: result.backup_path.map(|path| path.display().to_string()),
        before_hash: result.before_hash,
        after_hash: result.after_hash,
        restart_required: true,
        model_count: result.model_count,
        selected_model: result.selected_model,
        model_labels: result.model_labels,
        warnings: result.warnings,
    })
}

#[tauri::command]
async fn restore_previous_codex_config() -> Result<Option<String>, String> {
    codex_config::restore_latest()
        .map(|path| path.map(|path| path.display().to_string()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn kimi_target_status() -> Result<kimi_target::KimiTargetStatus, String> {
    Ok(kimi_target::inspect_status())
}

#[tauri::command]
async fn preview_kimi_publish(
    state: State<'_, AppState>,
) -> Result<kimi_target::KimiPublishPreview, String> {
    let snapshot = state.snapshot.read().await;
    let base_url = snapshot
        .proxy
        .base_url
        .clone()
        .ok_or_else(|| "本地代理尚未启动".to_string())?;
    drop(snapshot);
    state.rebuild_runtime().await?;
    let catalog = state.catalog.read().await.clone();
    let routes = state.routes.read().await.clone();
    let secret = state.proxy.read().await.secret.clone();
    kimi_target::preview(&base_url, secret.as_str(), &catalog, &routes).map_err(|e| e.to_string())
}

/// Write-only publish (方案 B): no system proxy, no whole-host shield.
async fn kimi_publish_core(state: &AppState) -> Result<kimi_target::KimiPublishOutcome, String> {
    let snapshot = state.snapshot.read().await;
    let base_url = snapshot
        .proxy
        .base_url
        .clone()
        .ok_or_else(|| "本地代理尚未启动".to_string())?;
    drop(snapshot);
    state.rebuild_runtime().await?;
    let catalog = state.catalog.read().await.clone();
    let routes = state.routes.read().await.clone();
    let secret = state.proxy.read().await.secret.clone();
    let mut outcome = kimi_target::apply(&base_url, secret.as_str(), &catalog, &routes)
        .map_err(|e| e.to_string())?;
    // Ensure leftover whole-host shield is not left running from older builds.
    let _ = state.kimi_list_shield.stop().await;
    #[cfg(target_os = "macos")]
    {
        if proxy_points_at_spur_shield() {
            let _ = disable_macos_https_proxy();
            outcome
                .warnings
                .push("已关闭残留的系统代理（旧版整站拦截会弄挂 Kimi，方案 B 不再使用）。".into());
        }
    }
    outcome.warnings.push(
        "右下角若仍只有官方模型：请用路径拦截 DescribeKimiWorkConfig（docs/kimi-app-selective-block.md），勿整站拦 www.kimi.com。"
            .into(),
    );
    Ok(outcome)
}

#[tauri::command]
async fn apply_kimi_publish(
    state: State<'_, AppState>,
) -> Result<kimi_target::KimiPublishOutcome, String> {
    kimi_publish_core(&state).await
}

#[tauri::command]
async fn restore_kimi_publish(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let result = disable_kimi_publish(state).await?;
    Ok(if result.message.contains("备份") {
        Some(result.message)
    } else {
        None
    })
}

#[tauri::command]
async fn reapply_kimi_model_list(
    state: State<'_, AppState>,
) -> Result<kimi_target::KimiPublishOutcome, String> {
    kimi_publish_core(&state).await
}

/// One-shot 方案 B: write Kimi config only (no system proxy / whole-host block).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct KimiPublishToggleResult {
    enabled: bool,
    model_count: u32,
    model_labels: Vec<String>,
    shield_listen: Option<String>,
    proxy_ok: bool,
    message: String,
    warnings: Vec<String>,
}

#[tauri::command]
async fn enable_kimi_publish(
    state: State<'_, AppState>,
) -> Result<KimiPublishToggleResult, String> {
    let outcome = kimi_publish_core(&state).await?;
    let mut warnings = outcome.warnings;

    if let Err(err) = kimi_target::set_publish_active(true) {
        warnings.push(format!("写入启用标记失败：{err}"));
    }

    let listed = if outcome.model_labels.is_empty() {
        format!("{} 个模型", outcome.model_count)
    } else {
        outcome
            .model_labels
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let message = format!(
        "已启用（仅写盘）：写入 {listed} 到 Kimi 缓存/配置。\n\
         · 不会改系统代理，Kimi 应能正常打开。\n\
         · 右下角若仍只有官方列表：用路径拦截 DescribeKimiWorkConfig（见 docs/kimi-app-selective-block.md），再完全退出重开 Kimi。\n\
         · 禁止整站代理 www.kimi.com（会弄挂 Kimi）。\n\
         · 日常稳定多模型请用 Codex Review & Apply。"
    );

    Ok(KimiPublishToggleResult {
        enabled: true,
        model_count: outcome.model_count,
        model_labels: outcome.model_labels,
        shield_listen: None,
        proxy_ok: true, // true = "safe mode" / no broken system proxy
        message,
        warnings,
    })
}

/// One-shot: restore Kimi backup + clear residual shield/proxy + inactive flag.
#[tauri::command]
async fn disable_kimi_publish(
    state: State<'_, AppState>,
) -> Result<KimiPublishToggleResult, String> {
    let mut warnings = Vec::new();
    let restored = kimi_target::restore_latest().map_err(|e| e.to_string())?;
    if restored.is_none() {
        if let Err(err) = kimi_target::uninstall_spur_bits() {
            warnings.push(format!("清理 Spur 注入：{err}"));
        } else {
            warnings.push("无备份可恢复，已尝试移除 Spur 注入项。".into());
        }
    }

    let _ = state.kimi_list_shield.stop().await;

    #[cfg(target_os = "macos")]
    {
        if let Err(err) = disable_macos_https_proxy() {
            warnings.push(format!("关闭系统代理：{err}"));
        }
    }

    if let Err(err) = kimi_target::set_publish_active(false) {
        warnings.push(format!("清除启用标记失败：{err}"));
    }

    let message = match restored {
        Some(path) => {
            format!("已关闭发布：已恢复备份并清理残留代理。请完全退出并重开 Kimi。\n备份：{path}")
        }
        None => "已关闭发布：已清理 Spur 注入与残留代理。请完全退出并重开 Kimi。".into(),
    };

    Ok(KimiPublishToggleResult {
        enabled: false,
        model_count: 0,
        model_labels: Vec::new(),
        shield_listen: None,
        proxy_ok: false,
        message,
        warnings,
    })
}

#[tauri::command]
async fn kimi_list_shield_status(
    state: State<'_, AppState>,
) -> Result<kimi_list_shield::KimiListShieldStatus, String> {
    Ok(state.kimi_list_shield.status().await)
}

#[tauri::command]
async fn start_kimi_list_shield(
    state: State<'_, AppState>,
) -> Result<kimi_list_shield::KimiListShieldStatus, String> {
    state
        .kimi_list_shield
        .start()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_kimi_list_shield(
    state: State<'_, AppState>,
) -> Result<kimi_list_shield::KimiListShieldStatus, String> {
    state
        .kimi_list_shield
        .stop()
        .await
        .map_err(|e| e.to_string())
}

/// Legacy no-op: whole-host system proxy is forbidden (breaks Kimi). Path-only intercept only.
#[tauri::command]
async fn enable_kimi_list_shield_system_proxy(
    _state: State<'_, AppState>,
) -> Result<String, String> {
    Err(
        "已禁用：整站系统代理会弄挂 Kimi。请用路径拦截 DescribeKimiWorkConfig（docs/kimi-app-selective-block.md）。"
            .into(),
    )
}

#[tauri::command]
async fn disable_kimi_list_shield_system_proxy() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        disable_macos_https_proxy()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("非 macOS，无系统代理可关。".into())
    }
}

/// True if any primary service has HTTP(S) proxy → 127.0.0.1:17862–17894 (old list shield).
#[cfg(target_os = "macos")]
fn proxy_points_at_spur_shield() -> bool {
    for service in primary_network_services() {
        for flag in ["-getwebproxy", "-getsecurewebproxy"] {
            let Ok(output) = std::process::Command::new("networksetup")
                .args([flag, &service])
                .output()
            else {
                continue;
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let enabled = text.lines().any(|l| l.contains("Enabled: Yes"));
            let spur_host = text.lines().any(|l| l.contains("Server: 127.0.0.1"));
            let spur_port = text.lines().any(|l| {
                l.starts_with("Port:")
                    && l.split(':')
                        .nth(1)
                        .and_then(|p| p.trim().parse::<u16>().ok())
                        .is_some_and(|p| (17862..=17894).contains(&p))
            });
            if enabled && spur_host && spur_port {
                return true;
            }
        }
    }
    false
}

/// Clear residual system proxy left by older Spur builds (safe to call at startup).
pub fn clear_residual_kimi_system_proxy_if_needed() {
    #[cfg(target_os = "macos")]
    {
        if proxy_points_at_spur_shield() {
            match disable_macos_https_proxy() {
                Ok(msg) => tracing::warn!(%msg, "cleared residual Spur Kimi system proxy"),
                Err(err) => tracing::warn!(%err, "failed to clear residual system proxy"),
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn primary_network_services() -> Vec<String> {
    let output = std::process::Command::new("networksetup")
        .args(["-listallnetworkservices"])
        .output();
    let Ok(output) = output else {
        return vec!["Wi-Fi".into()];
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut services = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("An asterisk") || line.starts_with('*') {
            continue;
        }
        // Prefer common active interfaces first
        if line == "Wi-Fi"
            || line == "Ethernet"
            || line.contains("USB")
            || line.contains("Thunderbolt")
        {
            services.push(line.to_string());
        }
    }
    if services.is_empty() {
        services.push("Wi-Fi".into());
    }
    services
}

#[cfg(target_os = "macos")]
fn disable_macos_https_proxy() -> Result<String, String> {
    let services = primary_network_services();
    let mut ok = Vec::new();
    for service in &services {
        let _ = std::process::Command::new("networksetup")
            .args(["-setwebproxystate", service, "off"])
            .output();
        let _ = std::process::Command::new("networksetup")
            .args(["-setsecurewebproxystate", service, "off"])
            .output();
        ok.push(service.clone());
    }
    Ok(format!(
        "已尝试关闭 {} 的系统 HTTP/HTTPS 代理。",
        ok.join(", ")
    ))
}

#[tauri::command]
async fn list_model_routes(state: State<'_, AppState>) -> Result<Vec<ModelRouteSummary>, String> {
    state
        .storage
        .route_summaries()
        .await
        .map_err(|error| error.to_string())
}

async fn resolve_bearer_for_discover(
    state: &AppState,
    provider_id: &str,
    form_api_key: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(key) = form_api_key.map(str::trim).filter(|key| !key.is_empty()) {
        return Ok(Some(key.to_string()));
    }
    let credential = state
        .storage
        .first_healthy_credential(provider_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(credential) = credential else {
        return Ok(None);
    };
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| error.to_string())?;
    let secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;
    Ok(secret
        .api_key
        .or(secret.access_token)
        .or(secret.session_token))
}

async fn store_api_key_credential(
    state: &AppState,
    provider_id: &str,
    api_key: &str,
) -> Result<(), String> {
    let import_json = serde_json::json!({
        "provider": provider_id,
        "api_key": api_key,
    })
    .to_string();
    let Some(credential) = credentials::parse_json_import(&import_json)
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let credential = credential.assign_provider(provider_id);
    let id = Uuid::new_v4().to_string();
    let plaintext = Zeroizing::new(
        providers::credential_secret_json(&credential).map_err(|error| error.to_string())?,
    );
    let envelope = state
        .vault
        .encrypt(&id, 1, plaintext.as_slice())
        .map_err(|error| error.to_string())?;
    let envelope_json = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
    let inserted = state
        .storage
        .insert_credential(&credential, &id, &envelope_json)
        .await
        .map_err(|error| error.to_string())?;
    if inserted {
        let pool_id = state
            .storage
            .ensure_default_pool(provider_id)
            .await
            .map_err(|error| error.to_string())?;
        state
            .storage
            .add_pool_member(&pool_id, &id)
            .await
            .map_err(|error| error.to_string())?;
        let _ = state.storage.set_active_pool(provider_id, &pool_id).await;
    }
    // API Key form / config import path — mark channel for Overview badge.
    let _ = state
        .storage
        .set_provider_entry_category(provider_id, "api")
        .await;
    Ok(())
}

async fn publish_discovered_models(
    state: &AppState,
    provider: &ProviderSummary,
    models: &[providers::DiscoveredProviderModel],
    normalized_base: &str,
) -> Result<Vec<ModelRouteSummary>, String> {
    let records = models
        .iter()
        .map(|model| {
            providers::route_catalog_json(&provider.id, &provider.kind, &provider.name, model)
                .map(|catalog_json| (model.id.clone(), model.display_name.clone(), catalog_json))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    state
        .storage
        .replace_discovered_models(&provider.id, normalized_base, &records)
        .await
        .map_err(|error| error.to_string())?;
    // Discovery is already committed above. Runtime/snapshot refresh is secondary
    // work and must not keep the import wizard spinning forever. Newly discovered
    // routes are disabled by default, so a delayed refresh cannot route traffic to
    // stale entries; enabling/publishing a model rebuilds the runtime again.
    match tokio::time::timeout(std::time::Duration::from_secs(5), state.rebuild_runtime()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, provider_id = %provider.id, "runtime refresh after model discovery failed")
        }
        Err(_) => {
            tracing::warn!(provider_id = %provider.id, "runtime refresh after model discovery timed out")
        }
    }
    state
        .storage
        .route_summaries()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn inspect_opencode_go_credential() -> Result<OpenCodeGoCredentialStatus, String> {
    let path = opencode_go::auth_path().map_err(|error| error.to_string())?;
    let path_label = opencode_go::path_label(&path);
    match opencode_go::read_api_key(&path).await {
        Ok(_) => Ok(OpenCodeGoCredentialStatus {
            found: true,
            path_label,
            message: "已找到 OpenCode Go API 凭据".into(),
        }),
        Err(error) => Ok(OpenCodeGoCredentialStatus {
            found: false,
            path_label,
            message: error.to_string(),
        }),
    }
}

#[tauri::command]
async fn import_opencode_go_credential(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<CredentialSummary, String> {
    let provider = state
        .storage
        .get_provider(&provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".to_string())?;
    if provider.kind != "opencode-go" {
        return Err("该供应商不是 OpenCode Go".into());
    }
    let path = opencode_go::auth_path().map_err(|error| error.to_string())?;
    let key = opencode_go::read_api_key(&path)
        .await
        .map_err(|error| error.to_string())?;
    store_api_key_credential(&state, &provider_id, key.as_str()).await?;
    state
        .storage
        .list_credentials(Some(&provider_id))
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| "OpenCode Go 凭据保存失败".to_string())
}

#[tauri::command]
async fn discover_provider_models(
    state: State<'_, AppState>,
    provider_id: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<ModelRouteSummary>, String> {
    let provider = state
        .storage
        .get_provider(&provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".to_string())?;
    let form_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToOwned::to_owned);
    let official_openai = provider.kind == "openai" && base_url.trim().is_empty();
    let official_xai = provider.kind == "xai"
        && form_key.is_none()
        && (base_url.trim().is_empty()
            || providers::is_xai_official_host(&base_url)
            || provider
                .base_url
                .as_deref()
                .is_some_and(providers::is_xai_official_host)
            || matches!(
                provider.entry_category.as_deref(),
                Some("official") | Some("subscription") | Some("oauth")
            ));
    // Stamp entry channel before/alongside discovery so Overview badges stay accurate.
    // Do not overwrite an explicit JSON import (file) with "official" when re-fetching
    // via empty base_url (same path as official model discovery).
    if official_openai || official_xai {
        let existing = provider.entry_category.as_deref();
        let is_json_import =
            existing == Some("json") || existing == Some("pool") || existing == Some("config");
        if !is_json_import {
            let _ = state
                .storage
                .set_provider_entry_category(&provider_id, "official")
                .await;
        }
    } else if form_key.is_some() && (provider.kind != "openai" || !base_url.trim().is_empty()) {
        // Form API key only — do not clobber a prior JSON import stamp.
        let existing = provider.entry_category.as_deref();
        let is_json_import =
            existing == Some("json") || existing == Some("pool") || existing == Some("config");
        if !is_json_import {
            let _ = state
                .storage
                .set_provider_entry_category(&provider_id, "api")
                .await;
        }
    }
    let (models, normalized_base) = if official_openai {
        let credential = state
            .storage
            .first_healthy_credential(&provider_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "请先通过「OpenAI · 导入账号 JSON」或「OpenAI · 官方订阅」添加账号，再拉取模型"
                    .to_string()
            })?;
        let plaintext = state
            .vault
            .decrypt(&credential.id, &credential.secret_envelope)
            .map_err(|error| error.to_string())?;
        let mut secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
            .map_err(|error| format!("凭据数据损坏：{error}"))?;
        // Agent Identity path: sign models list with AgentAssertion.
        if let Some(mut agent_key) = openai_agent_identity::agent_key_from_secret(&secret) {
            let account_id = credential
                .account_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Agent Identity 账号缺少 account_id".to_string())?
                .to_string();
            if agent_key
                .task_id
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
            {
                let task_id = openai_agent_identity::register_agent_task(&agent_key)
                    .await
                    .map_err(|e| e.to_string())?;
                agent_key.task_id = Some(task_id.clone());
                secret.task_id = Some(task_id);
                if let Ok(json) =
                    providers::credential_secret_json(&credentials::CanonicalCredential {
                        kind: credentials::CredentialKind::AgentIdentity,
                        state: credentials::CredentialState::Refreshable,
                        provider_id: provider_id.clone(),
                        label: None,
                        email: None,
                        account_id: Some(account_id.clone()),
                        expires_at: None,
                        fingerprint: String::new(),
                        refreshable: true,
                        secret: secret.clone(),
                    })
                {
                    if let Ok(envelope) = state.vault.encrypt(&credential.id, 1, json.as_slice()) {
                        if let Ok(envelope_json) = serde_json::to_string(&envelope) {
                            let _ = state
                                .storage
                                .update_credential_secret(
                                    &credential.id,
                                    &envelope_json,
                                    None,
                                    Some(account_id.as_str()),
                                )
                                .await;
                        }
                    }
                }
            }
            let task_id = agent_key.task_id.clone().unwrap_or_default();
            let authorization =
                openai_agent_identity::authorization_header_for_agent_task(&agent_key, &task_id)
                    .map_err(|e| e.to_string())?;
            (
                providers::discover_official_models_with_authorization(&authorization, &account_id)
                    .await
                    .map_err(|error| error.to_string())?,
                "https://chatgpt.com/backend-api/codex".to_string(),
            )
        } else {
            let access_token = secret
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "OpenAI 官方账号缺少 access_token".to_string())?;
            let account_id = openai_oauth::ensure_chatgpt_account_id(
                access_token,
                secret.id_token.as_deref(),
                credential.account_id.as_deref(),
            )
            .await
            .map_err(|error| format!("OpenAI 官方账号缺少 account_id：{error}"))?;
            if credential.account_id.as_deref() != Some(account_id.as_str()) {
                if let Ok(json) = serde_json::to_vec(&serde_json::json!({
                    "access_token": secret.access_token,
                    "refresh_token": secret.refresh_token,
                    "id_token": secret.id_token,
                    "session_token": secret.session_token,
                    "api_key": secret.api_key,
                    "agent_runtime_id": secret.agent_runtime_id,
                    "agent_private_key": secret.agent_private_key,
                    "task_id": secret.task_id,
                })) {
                    if let Ok(envelope) = state.vault.encrypt(&credential.id, 1, json.as_slice()) {
                        if let Ok(envelope_json) = serde_json::to_string(&envelope) {
                            let _ = state
                                .storage
                                .update_credential_secret(
                                    &credential.id,
                                    &envelope_json,
                                    None,
                                    Some(account_id.as_str()),
                                )
                                .await;
                        }
                    }
                }
            }
            (
                providers::discover_official_models(access_token, &account_id)
                    .await
                    .map_err(|error| error.to_string())?,
                "https://chatgpt.com/backend-api/codex".to_string(),
            )
        }
    } else if official_xai {
        // Subscription OAuth: CLI chat proxy (not api.x.ai).
        let subscription_base = providers::resolve_xai_upstream_base(
            Some("official"),
            provider.base_url.as_deref().or(Some(base_url.as_str())),
        );
        let bearer = resolve_bearer_for_discover(&state, &provider_id, None).await?;
        let models = providers::discover_xai_models(&subscription_base, bearer.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        (models, subscription_base)
    } else {
        let effective_base = if base_url.trim().is_empty() {
            provider
                .base_url
                .clone()
                .or(provider.default_base_url.clone())
                .unwrap_or_default()
        } else {
            base_url.clone()
        };
        if effective_base.trim().is_empty() {
            return Err("请填写 Base URL".into());
        }
        let bearer = resolve_bearer_for_discover(&state, &provider_id, form_key.as_deref()).await?;
        if bearer.is_none() {
            return Err(
                "缺少 API Key。请到「API 配置」填写，或先在「导入 JSON」导入账号后再拉取。".into(),
            );
        }
        let models = if provider.kind == "xai" {
            providers::discover_xai_models(&effective_base, bearer.as_deref())
                .await
                .map_err(|error| error.to_string())?
        } else {
            providers::discover_models(&provider.kind, &effective_base, bearer.as_deref())
                .await
                .map_err(|error| error.to_string())?
        };
        let normalized_base =
            providers::normalize_base_url(&effective_base).map_err(|error| error.to_string())?;
        (models, normalized_base)
    };
    if let Some(key) = form_key.as_deref() {
        if provider.kind != "openai" || !base_url.trim().is_empty() {
            store_api_key_credential(&state, &provider_id, key).await?;
        }
    }
    publish_discovered_models(&state, &provider, &models, &normalized_base).await
}

#[tauri::command]
async fn import_provider_config_json(
    state: State<'_, AppState>,
    provider_id: String,
    input: String,
) -> Result<Vec<ModelRouteSummary>, String> {
    let provider = state
        .storage
        .get_provider(&provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".to_string())?;
    let fallback = provider
        .default_base_url
        .as_deref()
        .or(provider.base_url.as_deref());
    let config = providers::parse_provider_config_json_with_fallback(&input, fallback)
        .map_err(|error| error.to_string())?;
    if let Some(api_key) = config.api_key.as_deref() {
        store_api_key_credential(&state, &provider_id, api_key).await?;
    }
    // Provider config file import is always "json" (not form-filled API).
    // store_api_key_credential stamps "api"; override after import.
    state
        .storage
        .set_provider_entry_category(&provider_id, "json")
        .await
        .map_err(|error| error.to_string())?;
    let models = if config.models.is_empty() {
        let bearer = resolve_bearer_for_discover(&state, &provider_id, config.api_key.as_deref())
            .await?
            .ok_or_else(|| {
                "供应商配置未包含 models，也缺少 api_key，无法拉取模型列表".to_string()
            })?;
        providers::discover_models(&provider.kind, &config.base_url, Some(bearer.as_str()))
            .await
            .map_err(|error| error.to_string())?
    } else {
        config.models
    };
    publish_discovered_models(&state, &provider, &models, &config.base_url).await
}

#[tauri::command]
async fn create_provider_instance(
    state: State<'_, AppState>,
    kind: String,
    name: Option<String>,
) -> Result<ProviderSummary, String> {
    let id = state
        .storage
        .create_provider_instance(&kind, name.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .get_provider(&id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "创建供应商后读取失败".to_string())
}

#[tauri::command]
async fn start_openai_device_login(
    state: State<'_, AppState>,
) -> Result<openai_oauth::DeviceLoginStart, String> {
    state.openai_oauth.start_device_login().await
}

#[tauri::command]
async fn poll_openai_device_login(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<openai_oauth::DeviceLoginPoll, String> {
    state.openai_oauth.poll_device_login(&device_code).await
}

#[tauri::command]
async fn cancel_openai_device_login(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<(), String> {
    state.openai_oauth.cancel_device_login(&device_code).await;
    Ok(())
}

/// Start browser PKCE login (primary official-subscription path).
/// Returns only the authorize URL — tokens never cross IPC.
#[tauri::command]
async fn start_openai_browser_login(
    app: AppHandle,
    state: State<'_, AppState>,
    name: Option<String>,
) -> Result<openai_oauth::BrowserLoginStart, String> {
    stop_openai_oauth_listener(&state).await;
    state.openai_oauth.clear_browser_login().await;

    let (listener, port) = bind_oauth_callback_listener(openai_oauth::DEFAULT_OAUTH_REDIRECT_PORT)?;
    let (prepared, pending) = state.openai_oauth.prepare_browser_login(port, name).await?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    {
        let mut guard = state.openai_oauth_listener.lock().await;
        *guard = Some(shutdown_tx);
    }

    let app_handle = app.clone();
    let pending_for_thread = pending.clone();
    std::thread::Builder::new()
        .name("openai-oauth-callback".into())
        .spawn(move || {
            run_openai_oauth_callback_listener(
                app_handle,
                listener,
                pending_for_thread,
                shutdown_rx,
            );
        })
        .map_err(|e| format!("无法启动登录回调监听：{e}"))?;

    Ok(prepared)
}

#[tauri::command]
async fn cancel_openai_browser_login(state: State<'_, AppState>) -> Result<(), String> {
    stop_openai_oauth_listener(&state).await;
    state.openai_oauth.clear_browser_login().await;
    Ok(())
}

/// Manual fallback: paste callback URL if localhost redirect is blocked.
#[tauri::command]
async fn complete_openai_oauth_callback_url(
    app: AppHandle,
    state: State<'_, AppState>,
    callback_url: String,
) -> Result<OpenAiLoginComplete, String> {
    let pending = state
        .openai_oauth
        .peek_pending_browser()
        .await
        .ok_or_else(|| "请先打开授权页面".to_string())?;
    let tokens = state
        .openai_oauth
        .complete_browser_callback(&callback_url)
        .await?;
    let display_name = pending.display_name.clone();
    stop_openai_oauth_listener(&state).await;
    let result = complete_official_login(&state, display_name, tokens).await?;
    let _ = app.emit(
        "openai-oauth-finished",
        OpenAiOAuthFinishedEvent::from_complete(&result),
    );
    Ok(result)
}

#[tauri::command]
async fn open_external_url(url: String) -> Result<(), String> {
    open_url_in_browser(&url)
}

async fn stop_openai_oauth_listener(state: &AppState) {
    let sender = {
        let mut guard = state.openai_oauth_listener.lock().await;
        guard.take()
    };
    if let Some(tx) = sender {
        let _ = tx.send(());
    }
    // Best-effort wake any listener stuck in accept().
    let _ = std::net::TcpStream::connect(("127.0.0.1", openai_oauth::DEFAULT_OAUTH_REDIRECT_PORT));
}

fn bind_oauth_callback_listener(preferred_port: u16) -> Result<(TcpListener, u16), String> {
    match TcpListener::bind(("127.0.0.1", preferred_port)) {
        Ok(listener) => {
            listener
                .set_nonblocking(false)
                .map_err(|e| format!("配置登录回调端口失败：{e}"))?;
            Ok((listener, preferred_port))
        }
        Err(_) => {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .map_err(|e| format!("无法绑定登录回调端口：{e}"))?;
            listener
                .set_nonblocking(false)
                .map_err(|e| format!("配置登录回调端口失败：{e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("读取回调端口失败：{e}"))?
                .port();
            Ok((listener, port))
        }
    }
}

fn run_openai_oauth_callback_listener(
    app: AppHandle,
    listener: TcpListener,
    pending: openai_oauth::PendingBrowserLogin,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    // Small accept timeout loop so cancel can stop us.
    let _ = listener.set_nonblocking(true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = app.emit(
                "openai-oauth-finished",
                OpenAiOAuthFinishedEvent::error("登录已超时，请重新开始"),
            );
            break;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let path = match read_http_request_path(&mut stream) {
                    Ok(p) => p,
                    Err(error) => {
                        let _ = write_http_html(
                            &mut stream,
                            "400 Bad Request",
                            &openai_oauth::oauth_error_html(&error),
                        );
                        continue;
                    }
                };
                if path.starts_with("/__codex_spur_oauth_cancel") {
                    break;
                }
                if !path.starts_with("/auth/callback") {
                    let _ = write_http_html(
                        &mut stream,
                        "404 Not Found",
                        &openai_oauth::oauth_error_html("未知回调路径"),
                    );
                    continue;
                }
                let callback_url = match build_callback_url(&pending.redirect_uri, &path) {
                    Ok(url) => url,
                    Err(error) => {
                        let _ = write_http_html(
                            &mut stream,
                            "400 Bad Request",
                            &openai_oauth::oauth_error_html(&error),
                        );
                        let _ = app.emit(
                            "openai-oauth-finished",
                            OpenAiOAuthFinishedEvent::error(&error),
                        );
                        break;
                    }
                };

                let app_for_async = app.clone();
                let display_name = pending.display_name.clone();
                let pending_state = pending.state.clone();
                let result = tauri::async_runtime::block_on(async {
                    let state = app_for_async.state::<AppState>();
                    let tokens = state
                        .openai_oauth
                        .complete_browser_callback(&callback_url)
                        .await?;
                    let complete =
                        complete_official_login(state.inner(), display_name, tokens).await?;
                    state
                        .openai_oauth
                        .clear_browser_if_state_matches(&pending_state)
                        .await;
                    Ok::<OpenAiLoginComplete, String>(complete)
                });

                match result {
                    Ok(complete) => {
                        let _ = write_http_html(
                            &mut stream,
                            "200 OK",
                            &openai_oauth::oauth_success_html(),
                        );
                        let _ = app.emit(
                            "openai-oauth-finished",
                            OpenAiOAuthFinishedEvent::from_complete(&complete),
                        );
                    }
                    Err(error) => {
                        let _ = write_http_html(
                            &mut stream,
                            "400 Bad Request",
                            &openai_oauth::oauth_error_html(&error),
                        );
                        let _ = app.emit(
                            "openai-oauth-finished",
                            OpenAiOAuthFinishedEvent::error(&error),
                        );
                    }
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        }
    }
    // Drop listener; clear manager pending if still ours.
    let app2 = app.clone();
    let state_token = pending.state.clone();
    tauri::async_runtime::block_on(async move {
        let state = app2.state::<AppState>();
        state
            .openai_oauth
            .clear_browser_if_state_matches(&state_token)
            .await;
        let mut guard = state.openai_oauth_listener.lock().await;
        *guard = None;
    });
}

fn read_http_request_path(stream: &mut impl Read) -> Result<String, String> {
    let mut buf = [0u8; 8192];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("读取回调请求失败：{e}"))?;
    if n == 0 {
        return Err("空回调请求".into());
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let first_line = text.lines().next().unwrap_or_default();
    // GET /auth/callback?code=... HTTP/1.1
    let mut parts = first_line.split_whitespace();
    let _method = parts.next();
    let path = parts
        .next()
        .ok_or_else(|| "无法解析回调请求路径".to_string())?;
    Ok(path.to_string())
}

fn build_callback_url(redirect_uri: &str, request_path: &str) -> Result<String, String> {
    let base = url::Url::parse(redirect_uri).map_err(|e| format!("redirect_uri 无效：{e}"))?;
    let request = url::Url::parse(&format!("http://localhost{request_path}"))
        .map_err(|e| format!("回调路径无效：{e}"))?;
    let mut out = base;
    out.set_path(request.path());
    out.set_query(request.query());
    Ok(out.to_string())
}

fn write_http_html(stream: &mut impl Write, status: &str, body: &str) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| format!("写入回调响应失败：{e}"))?;
    let _ = stream.flush();
    Ok(())
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiOAuthFinishedEvent {
    ok: bool,
    provider: Option<ProviderSummary>,
    model_count: u32,
    model_error: Option<String>,
    message: Option<String>,
}

impl OpenAiOAuthFinishedEvent {
    fn from_complete(complete: &OpenAiLoginComplete) -> Self {
        Self {
            ok: true,
            provider: Some(complete.provider.clone()),
            model_count: complete.model_count,
            model_error: complete.model_error.clone(),
            message: None,
        }
    }

    fn error(message: &str) -> Self {
        Self {
            ok: false,
            provider: None,
            model_count: 0,
            model_error: None,
            message: Some(message.to_string()),
        }
    }
}

fn open_url_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("无法打开浏览器：{e}"))?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map_err(|e| format!("无法打开浏览器：{e}"))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("无法打开浏览器：{e}"))?;
        Ok(())
    }
}

struct OAuthTokenStoreInput<'a> {
    access_token: &'a str,
    refresh_token: Option<&'a str>,
    id_token: Option<&'a str>,
    account_id: &'a str,
    email: Option<&'a str>,
    expires_at: Option<i64>,
}

async fn store_oauth_tokens_credential(
    state: &AppState,
    provider_id: &str,
    tokens: OAuthTokenStoreInput<'_>,
) -> Result<(), String> {
    let mut import = serde_json::json!({
        "provider": provider_id,
        "access_token": tokens.access_token,
        "account_id": tokens.account_id,
    });
    if let Some(refresh) = tokens.refresh_token {
        import["refresh_token"] = serde_json::Value::String(refresh.to_string());
    }
    if let Some(id_token) = tokens.id_token {
        import["id_token"] = serde_json::Value::String(id_token.to_string());
    }
    if let Some(email) = tokens.email {
        import["email"] = serde_json::Value::String(email.to_string());
    }
    if let Some(expires_at) = tokens.expires_at {
        import["expires_at"] = serde_json::Value::Number(expires_at.into());
    }
    let credentials =
        credentials::parse_json_import(&import.to_string()).map_err(|error| error.to_string())?;
    for credential in credentials {
        let credential = credential.assign_provider(provider_id);
        let id = Uuid::new_v4().to_string();
        let plaintext = Zeroizing::new(
            providers::credential_secret_json(&credential).map_err(|error| error.to_string())?,
        );
        let envelope = state
            .vault
            .encrypt(&id, 1, plaintext.as_slice())
            .map_err(|error| error.to_string())?;
        let envelope_json = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
        let inserted = state
            .storage
            .insert_credential(&credential, &id, &envelope_json)
            .await
            .map_err(|error| error.to_string())?;
        if inserted {
            let pool_id = state
                .storage
                .ensure_default_pool(provider_id)
                .await
                .map_err(|error| error.to_string())?;
            state
                .storage
                .add_pool_member(&pool_id, &id)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiLoginComplete {
    provider: ProviderSummary,
    model_count: u32,
    model_error: Option<String>,
}

/// Create an OpenAI instance from completed OAuth tokens and fetch models.
async fn complete_official_login(
    state: &AppState,
    name: Option<String>,
    tokens: openai_oauth::DeviceLoginTokens,
) -> Result<OpenAiLoginComplete, String> {
    let id = state
        .storage
        .create_provider_instance("openai", name.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    store_oauth_tokens_credential(
        state,
        &id,
        OAuthTokenStoreInput {
            access_token: &tokens.access_token,
            refresh_token: tokens.refresh_token.as_deref(),
            id_token: tokens.id_token.as_deref(),
            account_id: &tokens.account_id,
            email: tokens.email.as_deref(),
            expires_at: tokens.expires_at,
        },
    )
    .await?;
    state
        .storage
        .set_provider_entry_category(&id, "official")
        .await
        .map_err(|error| error.to_string())?;
    let official_base = "https://chatgpt.com/backend-api/codex";
    let provider = state
        .storage
        .get_provider(&id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "创建供应商后读取失败".to_string())?;
    let model_result =
        providers::discover_official_models(&tokens.access_token, &tokens.account_id).await;
    match model_result {
        Ok(models) => {
            let routes =
                publish_discovered_models(state, &provider, &models, official_base).await?;
            let provider = state
                .storage
                .get_provider(&id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "供应商不存在".to_string())?;
            Ok(OpenAiLoginComplete {
                provider,
                model_count: routes
                    .iter()
                    .filter(|route| route.provider_id == id)
                    .count() as u32,
                model_error: None,
            })
        }
        Err(error) => {
            let _ = state
                .storage
                .replace_discovered_models(&id, official_base, &[])
                .await;
            let provider = state
                .storage
                .get_provider(&id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "供应商不存在".to_string())?;
            Ok(OpenAiLoginComplete {
                provider,
                model_count: 0,
                model_error: Some(format!(
                    "账号已登录并保存，模型拉取失败：{error}。可稍后在编辑页重试。"
                )),
            })
        }
    }
}

/// Create an OpenAI instance from completed device-login tokens and fetch models.
#[tauri::command]
async fn complete_openai_device_login(
    state: State<'_, AppState>,
    name: Option<String>,
    tokens: openai_oauth::DeviceLoginTokens,
) -> Result<OpenAiLoginComplete, String> {
    complete_official_login(&state, name, tokens).await
}

#[tauri::command]
async fn start_xai_device_login(
    state: State<'_, AppState>,
) -> Result<xai_oauth::DeviceLoginStart, String> {
    state.xai_oauth.start_device_login().await
}

#[tauri::command]
async fn poll_xai_device_login(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<xai_oauth::DeviceLoginPoll, String> {
    state.xai_oauth.poll_device_login(&device_code).await
}

#[tauri::command]
async fn cancel_xai_device_login(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<(), String> {
    state.xai_oauth.cancel_device_login(&device_code).await;
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct XaiLoginComplete {
    provider: ProviderSummary,
    model_count: u32,
    model_error: Option<String>,
}

/// Create a Grok / xAI instance from completed device-login tokens and fetch models.
#[tauri::command]
async fn complete_xai_device_login(
    state: State<'_, AppState>,
    name: Option<String>,
    tokens: xai_oauth::DeviceLoginTokens,
) -> Result<XaiLoginComplete, String> {
    let id = state
        .storage
        .create_provider_instance("xai", name.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    store_oauth_tokens_credential(
        &state,
        &id,
        OAuthTokenStoreInput {
            access_token: &tokens.access_token,
            refresh_token: tokens.refresh_token.as_deref(),
            id_token: tokens.id_token.as_deref(),
            account_id: &tokens.account_id,
            email: tokens.email.as_deref(),
            expires_at: tokens.expires_at,
        },
    )
    .await?;
    state
        .storage
        .set_provider_entry_category(&id, "official")
        .await
        .map_err(|error| error.to_string())?;
    // OAuth SuperGrok / Grok CLI subscription traffic uses the CLI chat proxy.
    let official_base = providers::XAI_CLI_SUBSCRIPTION_BASE;
    let provider = state
        .storage
        .get_provider(&id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "创建供应商后读取失败".to_string())?;
    let model_result =
        providers::discover_xai_models(official_base, Some(tokens.access_token.as_str())).await;
    match model_result {
        Ok(models) => {
            let routes =
                publish_discovered_models(&state, &provider, &models, official_base).await?;
            let provider = state
                .storage
                .get_provider(&id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "供应商不存在".to_string())?;
            Ok(XaiLoginComplete {
                provider,
                model_count: routes
                    .iter()
                    .filter(|route| route.provider_id == id)
                    .count() as u32,
                model_error: None,
            })
        }
        Err(error) => {
            // Still publish curated fallback so the instance is usable offline.
            let fallback = providers::xai_subscription_models();
            match publish_discovered_models(&state, &provider, &fallback, official_base).await {
                Ok(routes) => {
                    let provider = state
                        .storage
                        .get_provider(&id)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "供应商不存在".to_string())?;
                    Ok(XaiLoginComplete {
                        provider,
                        model_count: routes
                            .iter()
                            .filter(|route| route.provider_id == id)
                            .count() as u32,
                        model_error: Some(format!("已用内置 Grok 目录；在线模型列表失败：{error}")),
                    })
                }
                Err(publish_err) => {
                    let provider = state
                        .storage
                        .get_provider(&id)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "供应商不存在".to_string())?;
                    Ok(XaiLoginComplete {
                        provider,
                        model_count: 0,
                        model_error: Some(format!(
                            "账号已登录并保存，模型写入失败：{publish_err}（发现错误：{error}）"
                        )),
                    })
                }
            }
        }
    }
}

#[tauri::command]
async fn delete_provider_instance(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), String> {
    state
        .storage
        .delete_provider_instance(&provider_id)
        .await
        .map_err(|error| error.to_string())?;
    // Storage delete already committed; do not surface rebuild failures as "delete failed".
    if let Err(err) = state.rebuild_runtime().await {
        tracing::warn!(%err, provider_id = %provider_id, "rebuild_runtime after delete_provider_instance failed");
    }
    // Keep ~/.codex model-catalog.json in sync. Otherwise Desktop keeps showing
    // ghost models (e.g. "723 · GPT-5.6-Sol") whose provider/credentials are gone,
    // and turns fail with no_upstream_credential / Unauthorized.
    if let Err(err) = republish_codex_catalog_best_effort(&state).await {
        tracing::warn!(%err, provider_id = %provider_id, "republish after delete_provider_instance failed");
    }
    Ok(())
}

/// Best-effort rewrite of the on-disk Codex catalog from current enabled routes.
/// Does not fail the caller; requires a known proxy base URL + bearer.
async fn republish_codex_catalog_best_effort(state: &AppState) -> Result<(), String> {
    let (base_url, secret) = {
        let snapshot = state.snapshot.read().await;
        let base = snapshot
            .proxy
            .base_url
            .clone()
            .ok_or_else(|| "proxy base_url unavailable".to_string())?;
        drop(snapshot);
        let proxy = state.proxy.read().await;
        (base, proxy.secret.clone())
    };
    state.rebuild_runtime().await?;
    let catalog = state.catalog.read().await.clone();
    let result = codex_config::apply(&base_url, &secret, &catalog).map_err(|e| e.to_string())?;
    let _ = state
        .storage
        .record_apply_revision(
            &Uuid::new_v4().to_string(),
            &result.catalog_path.display().to_string(),
            &result.config_path.display().to_string(),
            result.before_hash.as_deref(),
            &result.after_hash,
            "applied",
        )
        .await;
    {
        let mut snapshot = state.snapshot.write().await;
        snapshot.published_models = result.model_count;
        snapshot.proxy.catalog_revision = format!("models-{}", result.model_count);
    }
    Ok(())
}

#[tauri::command]
async fn rename_provider_instance(
    state: State<'_, AppState>,
    provider_id: String,
    name: String,
) -> Result<ProviderSummary, String> {
    state
        .storage
        .rename_provider_instance(&provider_id, &name)
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .get_provider(&provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".to_string())
}

#[tauri::command]
async fn rename_credential(
    state: State<'_, AppState>,
    credential_id: String,
    label: String,
) -> Result<CredentialSummary, String> {
    state
        .storage
        .rename_credential(&credential_id, &label)
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .list_credentials(None)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|item| item.id == credential_id)
        .ok_or_else(|| "账号不存在".to_string())
}

#[tauri::command]
async fn set_active_pool(
    state: State<'_, AppState>,
    provider_id: String,
    pool_id: String,
) -> Result<(), String> {
    state
        .storage
        .set_active_pool(&provider_id, &pool_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_model_enabled(
    state: State<'_, AppState>,
    route_id: String,
    enabled: bool,
) -> Result<Vec<ModelRouteSummary>, String> {
    state
        .storage
        .set_route_enabled(&route_id, enabled)
        .await
        .map_err(|error| error.to_string())?;
    // Keep SQLite catalog_json healed when toggling (cheap; avoids stale camelCase).
    if let Ok(routes) = state.storage.list_routes(false).await {
        if let Some(route) = routes.iter().find(|route| route.id == route_id) {
            if let Ok(healed) = catalog::heal_stored_catalog_json(route) {
                let _ = state
                    .storage
                    .update_route_catalog_json(&route_id, &healed)
                    .await;
            }
        }
    }
    state.rebuild_runtime().await?;
    {
        let mut snapshot = state.snapshot.write().await;
        if snapshot.binding.state == "applied" {
            let msg = "模型选择已变更：请再次点击 Review & Apply，然后完全退出 ChatGPT 再打开，右下角才会刷新。";
            if !snapshot.attention_items.iter().any(|item| item == msg) {
                snapshot.attention_items.push(msg.into());
            }
        }
    }
    list_model_routes(state).await
}

/// Insert credential; returns `Some(id)` when a new row was written.
async fn insert_canonical_credential(
    state: &AppState,
    provider_id: &str,
    credential: credentials::CanonicalCredential,
) -> Result<Option<String>, String> {
    let credential = credential.assign_provider(provider_id);
    let id = Uuid::new_v4().to_string();
    let plaintext = Zeroizing::new(
        providers::credential_secret_json(&credential).map_err(|error| error.to_string())?,
    );
    let envelope = state
        .vault
        .encrypt(&id, 1, plaintext.as_slice())
        .map_err(|error| error.to_string())?;
    let envelope_json = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
    let inserted = state
        .storage
        .insert_credential(&credential, &id, &envelope_json)
        .await
        .map_err(|error| error.to_string())?;
    if inserted {
        let pool_id = state
            .storage
            .ensure_default_pool(provider_id)
            .await
            .map_err(|error| error.to_string())?;
        state
            .storage
            .add_pool_member(&pool_id, &id)
            .await
            .map_err(|error| error.to_string())?;
        let _ = state.storage.set_active_pool(provider_id, &pool_id).await;
        return Ok(Some(id));
    }
    Ok(None)
}

/// Result of best-effort Agent Identity upgrade (Sub2API-style session→agent path).
struct AgentUpgradeOutcome {
    credential: credentials::CanonicalCredential,
    /// Present when stored as access-only because agent/register failed.
    access_only_reason: Option<String>,
}

/// Upgrade ChatGPT access/session credentials to durable Agent Identity when possible.
/// Preserves access/refresh/id/session tokens so official 5h/7d quota can still be queried.
///
/// Soft-fallback keeps the access token for re-export / quota, but Codex inference
/// requires Agent Identity (or real OAuth) — callers must not treat access-only as healthy.
async fn maybe_upgrade_to_agent_identity(
    credential: credentials::CanonicalCredential,
) -> Result<AgentUpgradeOutcome, String> {
    use credentials::CredentialKind;
    if credential.kind == CredentialKind::AgentIdentity {
        return Ok(AgentUpgradeOutcome {
            credential,
            access_only_reason: None,
        });
    }
    // Keep real OAuth (with refresh) as-is — Codex backend accepts these Bearer tokens.
    if credential.kind == CredentialKind::OAuth && credential.secret.has_refresh_token() {
        return Ok(AgentUpgradeOutcome {
            credential,
            access_only_reason: None,
        });
    }
    // API keys stay API keys.
    if credential.kind == CredentialKind::ApiKey {
        return Ok(AgentUpgradeOutcome {
            credential,
            access_only_reason: None,
        });
    }
    let Some(access) = credential
        .secret
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(AgentUpgradeOutcome {
            credential,
            access_only_reason: Some("session 缺少 accessToken，无法注册 Agent Identity".into()),
        });
    };
    let preserved = openai_agent_identity::PreservedUsageTokens {
        refresh_token: credential.secret.refresh_token.clone(),
        id_token: credential.secret.id_token.clone(),
        session_token: credential.secret.session_token.clone(),
        expires_at: credential.expires_at,
    };
    match openai_agent_identity::upgrade_access_token_to_agent_identity_with_tokens(
        &access,
        credential.email.clone(),
        credential.account_id.clone(),
        credential.label.clone(),
        preserved,
    )
    .await
    {
        Ok(mut upgraded) => {
            upgraded.provider_id = credential.provider_id;
            Ok(AgentUpgradeOutcome {
                credential: upgraded,
                access_only_reason: None,
            })
        }
        Err(error) => {
            // Fall back to storing the original access-only credential.
            // Codex backend rejects bare ChatGPT web accessTokens (401 Unauthorized);
            // Sub2API requires agent/register for the same reason.
            tracing::warn!(%error, "agent identity upgrade failed; storing access credential");
            let reason = format!(
                "Agent Identity 注册失败：{error}。ChatGPT Web Session 的 accessToken 不能直接调用 Codex backend；请重试 session、导入已有 agent_identity JSON，或使用官方 OAuth。"
            );
            Ok(AgentUpgradeOutcome {
                credential,
                access_only_reason: Some(reason),
            })
        }
    }
}

/// Apply post-import health/meta for a credential id.
async fn finalize_imported_credential(
    state: &AppState,
    credential_id: &str,
    credential: &credentials::CanonicalCredential,
    access_only_reason: Option<&str>,
) -> Result<(), String> {
    use credentials::CredentialKind;
    let kind = credential.kind.as_db_str();
    let state_str = match credential.state {
        credentials::CredentialState::Refreshable => "refreshable",
        credentials::CredentialState::AccessOnly => "access_only",
        credentials::CredentialState::Expired => "expired",
        credentials::CredentialState::ReauthRequired => "reauth_required",
        credentials::CredentialState::Disabled => "disabled",
        credentials::CredentialState::Unknown => "unknown",
    };
    let refreshable = credential.kind == CredentialKind::AgentIdentity
        || (credential.refreshable && credential.secret.has_refresh_token());
    state
        .storage
        .update_credential_auth_meta(
            credential_id,
            kind,
            state_str,
            refreshable,
            credential.email.as_deref(),
            credential.label.as_deref(),
            credential.expires_at,
        )
        .await
        .map_err(|error| error.to_string())?;

    if credential.kind == CredentialKind::AgentIdentity
        || (credential.kind == CredentialKind::OAuth && credential.secret.has_refresh_token())
        || credential.kind == CredentialKind::ApiKey
    {
        state
            .storage
            .heal_credential_after_import(credential_id)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    // Web session / access-only: keep visible but not selectable for Codex proxy.
    let msg = access_only_reason.unwrap_or(
        "ChatGPT Web Session 为 access-only：无法直连 Codex，需 Agent Identity 或官方 OAuth",
    );
    state
        .storage
        .mark_schedule_state(
            credential_id,
            scheduler::ScheduleState::AuthInvalid,
            false,
            Some(msg),
            None,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Merge usage tokens into an existing credential when account_id/email matches.
/// Re-import always retries Agent Identity upgrade when the row is still access-only
/// (previous soft-fallback left auth_invalid and never recovered — root of stuck 401).
async fn try_merge_usage_tokens_into_existing(
    state: &AppState,
    provider_id: &str,
    incoming: &credentials::CanonicalCredential,
    access_only_reason: Option<&str>,
) -> Result<bool, String> {
    let account_id = incoming
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let email = incoming
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if account_id.is_none() && email.is_none() {
        return Ok(false);
    }
    let Some(existing) = state
        .storage
        .find_credential_for_merge(provider_id, account_id, email)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(false);
    };
    let plaintext = state
        .vault
        .decrypt(&existing.id, &existing.secret_envelope)
        .map_err(|error| error.to_string())?;
    let mut secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;

    let mut changed = false;
    let take = |dst: &mut Option<String>, src: &Option<String>| {
        if let Some(value) = src
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        {
            if dst.as_deref() != Some(value.as_str()) {
                *dst = Some(value);
                return true;
            }
        }
        false
    };
    changed |= take(&mut secret.access_token, &incoming.secret.access_token);
    changed |= take(&mut secret.refresh_token, &incoming.secret.refresh_token);
    changed |= take(&mut secret.id_token, &incoming.secret.id_token);
    changed |= take(&mut secret.session_token, &incoming.secret.session_token);
    // Prefer incoming agent fields when existing row has none (do not rotate keys).
    if !secret.is_agent_identity() {
        changed |= take(
            &mut secret.agent_runtime_id,
            &incoming.secret.agent_runtime_id,
        );
        changed |= take(
            &mut secret.agent_private_key,
            &incoming.secret.agent_private_key,
        );
        changed |= take(&mut secret.task_id, &incoming.secret.task_id);
    }

    // Even if tokens look identical, re-import must re-apply meta/health (auth_invalid stuck).
    let next_account = account_id
        .map(ToOwned::to_owned)
        .or(existing.account_id.clone());
    if changed || existing.account_id.as_deref() != account_id {
        persist_credential_secret(
            state,
            &existing.id,
            &secret,
            incoming.expires_at.or(existing.expires_at),
            next_account.as_deref(),
        )
        .await?;
    } else {
        // Tokens unchanged: still rewrite secret so expires/account stay current.
        persist_credential_secret(
            state,
            &existing.id,
            &secret,
            incoming.expires_at.or(existing.expires_at),
            next_account.as_deref(),
        )
        .await?;
    }

    // Build a view of the post-merge credential for finalize (kind/state from incoming).
    let mut merged = incoming.clone();
    merged.secret = secret;
    if merged.secret.is_agent_identity() {
        merged.kind = credentials::CredentialKind::AgentIdentity;
        merged.state = credentials::CredentialState::Refreshable;
        merged.refreshable = true;
    }
    finalize_imported_credential(
        state,
        &existing.id,
        &merged,
        if merged.secret.is_agent_identity() {
            None
        } else {
            access_only_reason
        },
    )
    .await?;
    Ok(true)
}

#[tauri::command]
async fn import_credentials_json(
    state: State<'_, AppState>,
    provider_id: String,
    input: String,
) -> Result<Vec<CredentialSummary>, String> {
    let credentials = credentials::parse_json_import(&input).map_err(|error| error.to_string())?;
    let mut any_changed = false;
    for credential in credentials {
        // Upgrade first so merge can write Agent Identity into an access-only row.
        let outcome = if credential.kind == credentials::CredentialKind::AgentIdentity
            || credential.kind == credentials::CredentialKind::ApiKey
            || (credential.kind == credentials::CredentialKind::OAuth
                && credential.secret.has_refresh_token())
        {
            AgentUpgradeOutcome {
                credential,
                access_only_reason: None,
            }
        } else {
            maybe_upgrade_to_agent_identity(credential).await?
        };
        // Merge usage tokens into an existing same-account row first so re-import
        // repairs quota without registering a second Agent Identity.
        if try_merge_usage_tokens_into_existing(
            &state,
            &provider_id,
            &outcome.credential,
            outcome.access_only_reason.as_deref(),
        )
        .await?
        {
            any_changed = true;
            continue;
        }
        if let Some(id) =
            insert_canonical_credential(&state, &provider_id, outcome.credential.clone()).await?
        {
            let assigned = outcome.credential.clone().assign_provider(&provider_id);
            finalize_imported_credential(
                &state,
                &id,
                &assigned,
                outcome.access_only_reason.as_deref(),
            )
            .await?;
            any_changed = true;
        }
    }
    if any_changed {
        // Account credentials file import → JSON badge (not browser 官方订阅).
        state
            .storage
            .set_provider_entry_category(&provider_id, "json")
            .await
            .map_err(|error| error.to_string())?;
        // Point instance at official Codex backend so models can be discovered immediately.
        let official_base = "https://chatgpt.com/backend-api/codex";
        let _ = state
            .storage
            .set_provider_base_url(&provider_id, Some(official_base))
            .await;
    }
    list_credentials(state, Some(provider_id)).await
}

/// Import a ChatGPT `/api/auth/session` dump.
///
/// Sub2API path: session accessToken → agent/register → Agent Identity.
/// Soft-fallback keeps access-only for visibility, but marks it unhealthy for Codex
/// (web accessToken alone gets `{"detail":"Unauthorized"}` from backend-api/codex).
#[tauri::command]
async fn import_session_json(
    state: State<'_, AppState>,
    provider_id: String,
    input: String,
) -> Result<Vec<CredentialSummary>, String> {
    let session = credentials::parse_session_import(&input).map_err(|error| error.to_string())?;
    if session
        .secret
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Err("session 缺少 accessToken".to_string());
    }
    // Always attempt Agent Identity first (including re-import of stuck access-only rows).
    let outcome = maybe_upgrade_to_agent_identity(session).await?;
    // Repair existing agent_identity / oauth / access-only rows by account_id.
    if try_merge_usage_tokens_into_existing(
        &state,
        &provider_id,
        &outcome.credential,
        outcome.access_only_reason.as_deref(),
    )
    .await?
    {
        state
            .storage
            .set_provider_entry_category(&provider_id, "json")
            .await
            .map_err(|error| error.to_string())?;
        let official_base = "https://chatgpt.com/backend-api/codex";
        let _ = state
            .storage
            .set_provider_base_url(&provider_id, Some(official_base))
            .await;
        return list_credentials(state, Some(provider_id)).await;
    }
    if let Some(id) =
        insert_canonical_credential(&state, &provider_id, outcome.credential.clone()).await?
    {
        let assigned = outcome.credential.clone().assign_provider(&provider_id);
        finalize_imported_credential(
            &state,
            &id,
            &assigned,
            outcome.access_only_reason.as_deref(),
        )
        .await?;
        state
            .storage
            .set_provider_entry_category(&provider_id, "json")
            .await
            .map_err(|error| error.to_string())?;
        let official_base = "https://chatgpt.com/backend-api/codex";
        let _ = state
            .storage
            .set_provider_base_url(&provider_id, Some(official_base))
            .await;
    }
    list_credentials(state, Some(provider_id)).await
}

#[tauri::command]
async fn delete_credential(
    state: State<'_, AppState>,
    credential_id: String,
) -> Result<DeleteCredentialResult, String> {
    let result = state
        .storage
        .delete_credential(&credential_id)
        .await
        .map_err(|error| error.to_string())?;
    // Storage delete already committed; do not surface rebuild failures as "delete failed".
    if let Err(err) = state.rebuild_runtime().await {
        tracing::warn!(%err, credential_id = %credential_id, "rebuild_runtime after delete_credential failed");
    }
    Ok(result)
}

#[tauri::command]
async fn list_credentials(
    state: State<'_, AppState>,
    provider_id: Option<String>,
) -> Result<Vec<CredentialSummary>, String> {
    state
        .storage
        .list_credentials(provider_id.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn test_account(
    state: State<'_, AppState>,
    credential_id: String,
    model_id: String,
) -> Result<(), String> {
    let credential = state
        .storage
        .get_credential(&credential_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    let provider = state
        .storage
        .get_provider(&credential.provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".to_string())?;
    let base_url = provider
        .base_url
        .or(provider.default_base_url)
        .ok_or_else(|| "供应商尚未配置 Base URL".to_string())?;
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| error.to_string())?;
    let secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;
    let result = providers::test_credential(&provider.kind, &base_url, &model_id, &secret).await;
    match result {
        Ok(()) => state
            .storage
            .mark_credential_health(&credential.id, true, None)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => {
            let message = error.to_string();
            state
                .storage
                .mark_credential_health(&credential.id, false, Some(&message))
                .await
                .map_err(|db_error| db_error.to_string())?;
            Err(message)
        }
    }
}

#[tauri::command]
async fn refresh_openai_quota(
    state: State<'_, AppState>,
    credential_id: String,
) -> Result<OpenAiQuotaSnapshot, String> {
    refresh_quota_for_state(&state, &credential_id).await
}

#[tauri::command]
async fn get_cached_openai_quota(
    state: State<'_, AppState>,
    credential_id: String,
) -> Result<Option<OpenAiQuotaSnapshot>, String> {
    state
        .storage
        .cached_quota_snapshot(&credential_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn consume_openai_reset_credit(
    state: State<'_, AppState>,
    credential_id: String,
    idempotency_key: String,
    confirmed: bool,
) -> Result<OpenAiQuotaSnapshot, String> {
    if !confirmed {
        return Err("消耗重置卡前必须明确确认".into());
    }
    Uuid::parse_str(&idempotency_key)
        .map_err(|_| "幂等键必须是稳定 UUID；超时后必须继续使用同一个键".to_string())?;
    let reserved = state
        .storage
        .reserve_reset_credit_action(&credential_id, &idempotency_key)
        .await
        .map_err(|error| error.to_string())?;
    if !reserved {
        return Err("该幂等键已经提交过。请刷新额度确认结果，不要生成新键重试。".into());
    }
    let (access_token, account_id) = quota_access(&state, &credential_id).await?;
    match quota::consume_reset_credit(&access_token, &account_id, &idempotency_key).await {
        Ok(payload) => {
            let result_json = serde_json::to_string(&payload).map_err(|error| error.to_string())?;
            state
                .storage
                .finish_reset_credit_action(&idempotency_key, "completed", Some(&result_json))
                .await
                .map_err(|error| error.to_string())?;
            refresh_quota_for_state(&state, &credential_id).await
        }
        Err(error) => {
            let status = if error.is_ambiguous() {
                "ambiguous"
            } else {
                "failed"
            };
            state
                .storage
                .finish_reset_credit_action(&idempotency_key, status, None)
                .await
                .map_err(|db_error| db_error.to_string())?;
            if error.is_ambiguous() {
                Err(format!(
                    "{error}。结果不确定：请保留幂等键并刷新额度，禁止换新键重试。"
                ))
            } else {
                Err(error.to_string())
            }
        }
    }
}

#[tauri::command]
async fn create_account_pool(
    state: State<'_, AppState>,
    provider_id: String,
    name: String,
) -> Result<String, String> {
    state
        .storage
        .create_pool(&provider_id, &name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_account_pools(state: State<'_, AppState>) -> Result<Vec<AccountPoolSummary>, String> {
    state
        .storage
        .list_pools()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn add_account_to_pool(
    state: State<'_, AppState>,
    pool_id: String,
    credential_id: String,
) -> Result<(), String> {
    state
        .storage
        .add_pool_member(&pool_id, &credential_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_pool_member_ids(
    state: State<'_, AppState>,
    pool_id: String,
) -> Result<Vec<String>, String> {
    state
        .storage
        .list_pool_member_ids(&pool_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_pool_members_detailed(
    state: State<'_, AppState>,
    pool_id: String,
) -> Result<Vec<PoolMemberDetail>, String> {
    state
        .storage
        .list_pool_members_detailed(&pool_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn update_pool_member(
    state: State<'_, AppState>,
    pool_id: String,
    credential_id: String,
    weight: i64,
    priority: i64,
    enabled: bool,
    concurrency_limit: i64,
    upstream_cost_rate: Option<f64>,
) -> Result<(), String> {
    state
        .storage
        .update_pool_member(
            &pool_id,
            &credential_id,
            weight,
            priority,
            enabled,
            concurrency_limit,
            upstream_cost_rate,
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_provider_routing(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<Option<ProviderRouting>, String> {
    state
        .storage
        .get_provider_routing(&provider_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_provider_routing(
    state: State<'_, AppState>,
    provider_id: String,
    routing_mode: String,
    fixed_credential_id: Option<String>,
) -> Result<ProviderRouting, String> {
    state
        .storage
        .set_provider_routing(&provider_id, &routing_mode, fixed_credential_id.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .get_provider_routing(&provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商不存在".into())
}

#[tauri::command]
async fn get_pool_scheduler_config(
    state: State<'_, AppState>,
    pool_id: String,
) -> Result<PoolSchedulerConfig, String> {
    state
        .storage
        .get_pool_scheduler_config(&pool_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn update_pool_scheduler_config(
    state: State<'_, AppState>,
    pool_id: String,
    config: PoolSchedulerConfig,
) -> Result<PoolSchedulerConfig, String> {
    state
        .storage
        .update_pool_scheduler_config(&pool_id, &config)
        .await
        .map_err(|error| error.to_string())?;
    state
        .storage
        .get_pool_scheduler_config(&pool_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_proxy_request_events(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<ProxyRequestEvent>, String> {
    state
        .storage
        .list_proxy_request_events(limit.unwrap_or(100))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn clear_proxy_request_events(state: State<'_, AppState>) -> Result<(), String> {
    state
        .storage
        .clear_proxy_request_events()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_diagnostics_max_events(state: State<'_, AppState>) -> Result<i64, String> {
    state
        .storage
        .diagnostics_max_events()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_diagnostics_max_events(
    state: State<'_, AppState>,
    max_events: i64,
) -> Result<i64, String> {
    state
        .storage
        .set_diagnostics_max_events(max_events)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn remove_account_from_pool(
    state: State<'_, AppState>,
    pool_id: String,
    credential_id: String,
) -> Result<(), String> {
    state
        .storage
        .remove_pool_member(&pool_id, &credential_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn restart_proxy(state: State<'_, AppState>) -> Result<(), String> {
    state.restart_proxy().await
}

#[tauri::command]
async fn proxy_secret_available(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(!state.proxy.read().await.secret.is_empty())
}

#[tauri::command]
async fn inspect_credential_json(input: String) -> Result<Vec<CredentialImportSummary>, String> {
    credentials::parse_json_import(&input)
        .map(|items| items.into_iter().map(|item| item.summary()).collect())
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn keychain_ready(state: State<'_, AppState>) -> Result<bool, String> {
    let _ = &state.vault;
    Ok(true)
}

fn install_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 Codex Spur", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "本地代理：运行中", false, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启本地代理", true, None::<&str>)?;
    let quota = MenuItem::with_id(app, "quota", "刷新 OpenAI 额度", true, None::<&str>)?;
    let restore = MenuItem::with_id(app, "restore", "恢复上一次 Codex 配置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 Codex Spur", true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .item(&open)
        .item(&status)
        .separator()
        .item(&restart)
        .item(&quota)
        .item(&restore)
        .separator()
        .item(&quit)
        .build()?;
    // The full application icon has an opaque squircle background, which macOS
    // renders as a solid block in the menu bar when used as a template. Keep a
    // dedicated transparent glyph for the 18 pt menu-bar surface instead.
    let tray_icon = tauri::image::Image::new(include_bytes!("../icons/tray-icon.rgba"), 44, 44);
    let mut builder = TrayIconBuilder::with_id("codex-select")
        .tooltip("Codex Spur")
        .icon(tray_icon);
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }
    let mut builder = builder
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "restart" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<AppState>();
                    if let Err(error) = state.restart_proxy().await {
                        tracing::error!(%error, "failed to restart local proxy");
                    }
                });
            }
            "quota" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<AppState>();
                    // Credentials attach to UUID provider instances; filter by provider.kind.
                    match state.storage.list_credentials_for_kind("openai").await {
                        Ok(accounts) => {
                            for account in accounts {
                                if let Err(error) =
                                    refresh_quota_for_state(&state, &account.id).await
                                {
                                    tracing::warn!(
                                        account = %account.fingerprint_prefix,
                                        %error,
                                        "failed to refresh OpenAI quota"
                                    );
                                }
                            }
                        }
                        Err(error) => tracing::error!(%error, "failed to list OpenAI accounts"),
                    }
                });
            }
            "restore" => {
                tauri::async_runtime::spawn_blocking(|| {
                    if let Err(error) = codex_config::restore_latest() {
                        tracing::error!(%error, "failed to restore Codex configuration");
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let _tray = builder.build(app)?;
    Ok(())
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "codex_select=info".into()))
        .with_target(false)
        .compact()
        .init();

    let app = tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let state = tauri::async_runtime::block_on(AppState::bootstrap(data_dir))
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            app.manage(state);
            install_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            get_usage_snapshot,
            get_usage_dashboard,
            preview_codex_apply,
            apply_codex_config,
            restore_previous_codex_config,
            kimi_target_status,
            preview_kimi_publish,
            apply_kimi_publish,
            restore_kimi_publish,
            reapply_kimi_model_list,
            enable_kimi_publish,
            disable_kimi_publish,
            kimi_list_shield_status,
            start_kimi_list_shield,
            stop_kimi_list_shield,
            enable_kimi_list_shield_system_proxy,
            disable_kimi_list_shield_system_proxy,
            list_model_routes,
            discover_provider_models,
            inspect_opencode_go_credential,
            import_opencode_go_credential,
            import_provider_config_json,
            create_provider_instance,
            delete_provider_instance,
            rename_provider_instance,
            rename_credential,
            start_openai_device_login,
            poll_openai_device_login,
            cancel_openai_device_login,
            complete_openai_device_login,
            start_openai_browser_login,
            cancel_openai_browser_login,
            complete_openai_oauth_callback_url,
            start_xai_device_login,
            poll_xai_device_login,
            cancel_xai_device_login,
            complete_xai_device_login,
            open_external_url,
            set_active_pool,
            set_model_enabled,
            import_credentials_json,
            import_session_json,
            list_credentials,
            delete_credential,
            test_account,
            refresh_openai_quota,
            get_cached_openai_quota,
            consume_openai_reset_credit,
            create_account_pool,
            list_account_pools,
            add_account_to_pool,
            remove_account_from_pool,
            list_pool_member_ids,
            list_pool_members_detailed,
            update_pool_member,
            get_provider_routing,
            set_provider_routing,
            get_pool_scheduler_config,
            update_pool_scheduler_config,
            list_proxy_request_events,
            clear_proxy_request_events,
            get_diagnostics_max_events,
            set_diagnostics_max_events,
            restart_proxy,
            proxy_secret_available,
            inspect_credential_json,
            keychain_ready,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Codex Spur");
    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            let state = app.state::<AppState>();
            tauri::async_runtime::block_on(state.shutdown());
        }
    });
}

#[cfg(test)]
mod ops_import {
    use super::*;
    use std::path::PathBuf;

    /// One-shot local import: set SPUR_IMPORT_JSON (+ optional SPUR_DATA_DIR, SPUR_PROVIDER_NAME).
    /// Never prints secrets. Used by operators; skipped unless env is set.
    #[tokio::test]
    async fn import_credentials_json_from_env_path() {
        let Some(json_path) = std::env::var_os("SPUR_IMPORT_JSON") else {
            return;
        };
        let Some(data_dir) = std::env::var_os("SPUR_DATA_DIR").map(PathBuf::from) else {
            eprintln!("SPUR_DATA_DIR required with SPUR_IMPORT_JSON");
            return;
        };
        let name =
            std::env::var("SPUR_PROVIDER_NAME").unwrap_or_else(|_| "OpenAI · Web Session".into());
        let input = std::fs::read_to_string(&json_path).expect("read import json");
        let credentials = credentials::parse_json_import(&input).expect("parse import json");
        assert!(
            !credentials.is_empty(),
            "no credentials parsed from import file"
        );

        let vault = vault::SecretVault::load_or_create(&data_dir).expect("vault");
        let storage = storage::Storage::open(&data_dir).await.expect("storage");
        let provider_id = storage
            .create_provider_instance("openai", Some(name.as_str()))
            .await
            .expect("create provider");
        storage
            .set_provider_entry_category(&provider_id, "json")
            .await
            .expect("entry category");

        let mut inserted = 0u32;
        for credential in credentials {
            let credential = credential.assign_provider(&provider_id);
            let id = Uuid::new_v4().to_string();
            let plaintext = Zeroizing::new(
                providers::credential_secret_json(&credential).expect("secret json"),
            );
            let envelope = vault
                .encrypt(&id, 1, plaintext.as_slice())
                .expect("encrypt");
            let envelope_json = serde_json::to_string(&envelope).expect("envelope");
            let did = storage
                .insert_credential(&credential, &id, &envelope_json)
                .await
                .expect("insert");
            if did {
                inserted += 1;
                let pool_id = storage
                    .ensure_default_pool(&provider_id)
                    .await
                    .expect("pool");
                storage
                    .add_pool_member(&pool_id, &id)
                    .await
                    .expect("pool member");
            }
        }
        // Summary only — no tokens, no emails in full.
        eprintln!("import_ok provider_id={provider_id} inserted={inserted}");
        assert!(inserted > 0, "expected at least one inserted credential");
    }
}
