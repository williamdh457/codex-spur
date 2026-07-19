#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod codex_config;
mod credentials;
mod domain;
mod providers;
mod proxy;
mod quota;
mod storage;
mod vault;

use std::sync::Arc;

use credentials::{CredentialImportSummary, SecretMaterial};
use domain::{
    AccountPoolSummary, AppSnapshot, ApplyPreview, CodexApplyOutcome, CodexBindingStatus,
    CredentialSummary, ModelRouteSummary, OpenAiQuotaSnapshot, ProxyStatus,
};
use tauri::{
    menu::{MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
    Manager, State, WindowEvent,
};
use tokio::sync::RwLock;
use uuid::Uuid;
use zeroize::Zeroizing;

pub struct AppState {
    snapshot: RwLock<AppSnapshot>,
    pub(crate) catalog: catalog::SharedCatalog,
    routes: catalog::SharedRoutes,
    storage: Arc<storage::Storage>,
    proxy: RwLock<proxy::ProxyRuntime>,
    vault: Arc<vault::SecretVault>,
}

impl AppState {
    async fn bootstrap(data_dir: std::path::PathBuf) -> anyhow::Result<Self> {
        let storage = Arc::new(storage::Storage::open(&data_dir).await?);
        let vault = Arc::new(vault::SecretVault::load_or_create(&data_dir)?);
        let stored_routes = storage.list_routes(true).await?;
        let (catalog_value, route_values) = catalog::build_from_routes(&stored_routes);
        let catalog = Arc::new(RwLock::new(catalog_value));
        let routes = Arc::new(RwLock::new(route_values));
        let proxy = proxy::start(
            Arc::clone(&catalog),
            Arc::clone(&routes),
            Arc::clone(&storage),
            Arc::clone(&vault),
            17_861,
        )
        .await?;
        let base_url = format!("http://127.0.0.1:{}/v1", proxy.port);
        let providers = storage.list_providers().await?;
        let credentials = storage.list_credentials(None).await?;
        let published_models = catalog.read().await.models.len() as u32;
        let mut attention_items = Vec::new();
        if published_models == 0 {
            attention_items.push("添加供应商凭据并拉取模型后，才能应用到 Codex。".into());
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
                state: "not_applied".into(),
                codex_home: codex_config::codex_home().display().to_string(),
                provider_id: "codex_select".into(),
                catalog_path: codex_config::codex_home()
                    .join("codex-select")
                    .join("model-catalog.json")
                    .display()
                    .to_string(),
            },
            providers,
            published_models,
            healthy_accounts: credentials.iter().filter(|item| item.healthy).count() as u32,
            attention_items,
        };
        Ok(Self {
            snapshot: RwLock::new(snapshot),
            catalog,
            routes,
            storage,
            proxy: RwLock::new(proxy),
            vault,
        })
    }

    async fn rebuild_runtime(&self) -> Result<(), String> {
        let stored_routes = self
            .storage
            .list_routes(true)
            .await
            .map_err(|error| error.to_string())?;
        let (catalog_value, route_values) = catalog::build_from_routes(&stored_routes);
        let published_models = catalog_value.models.len() as u32;
        *self.catalog.write().await = catalog_value;
        *self.routes.write().await = route_values;
        let mut snapshot = self.snapshot.write().await;
        snapshot.published_models = published_models;
        snapshot.proxy.catalog_revision = format!("models-{published_models}");
        snapshot.providers = self
            .storage
            .list_providers()
            .await
            .map_err(|error| error.to_string())?;
        snapshot
            .attention_items
            .retain(|item| !item.contains("拉取模型"));
        if published_models == 0 {
            snapshot
                .attention_items
                .push("添加供应商凭据并拉取模型后，才能应用到 Codex。".into());
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
}

async fn quota_access(state: &AppState, credential_id: &str) -> Result<(String, String), String> {
    let credential = state
        .storage
        .get_credential(credential_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    if credential.provider_id != "openai" {
        return Err("额度接口仅适用于 OpenAI 官方订阅账号".into());
    }
    let account_id = credential
        .account_id
        .ok_or_else(|| "账号 JSON 缺少 account_id，无法查询官方订阅额度".to_string())?;
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| error.to_string())?;
    let secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;
    let access_token = secret
        .access_token
        .ok_or_else(|| "账号没有 access_token，无法查询官方订阅额度".to_string())?;
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
    Ok(state.proxy.read().await.metrics.snapshot())
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
    let mut snapshot = state.snapshot.write().await;
    snapshot.providers = providers;
    snapshot.healthy_accounts = credentials.iter().filter(|item| item.healthy).count() as u32;
    Ok(snapshot.clone())
}

#[tauri::command]
async fn preview_codex_apply(state: State<'_, AppState>) -> Result<ApplyPreview, String> {
    let snapshot = state.snapshot.read().await;
    let catalog_count = state.catalog.read().await.models.len() as u32;
    let base_url = snapshot
        .proxy
        .base_url
        .clone()
        .ok_or_else(|| "本地代理尚未启动".to_string())?;
    Ok(codex_config::preview(&base_url, catalog_count))
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
    let catalog = state.catalog.read().await.clone();
    let proxy = state.proxy.read().await;
    let result = codex_config::apply(&base_url, &proxy.secret, &catalog)
        .map_err(|error| error.to_string())?;
    state.snapshot.write().await.binding.state = "applied".into();
    Ok(CodexApplyOutcome {
        config_path: result.config_path.display().to_string(),
        catalog_path: result.catalog_path.display().to_string(),
        backup_path: result.backup_path.map(|path| path.display().to_string()),
        before_hash: result.before_hash,
        after_hash: result.after_hash,
        restart_required: true,
    })
}

#[tauri::command]
async fn restore_previous_codex_config() -> Result<Option<String>, String> {
    codex_config::restore_latest()
        .map(|path| path.map(|path| path.display().to_string()))
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn list_model_routes(state: State<'_, AppState>) -> Result<Vec<ModelRouteSummary>, String> {
    state
        .storage
        .route_summaries()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn discover_provider_models(
    state: State<'_, AppState>,
    provider_id: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<ModelRouteSummary>, String> {
    let api_key = api_key.map(Zeroizing::new);
    let (models, normalized_base) = if provider_id == "openai" && base_url.trim().is_empty() {
        let credential = state
            .storage
            .first_healthy_credential("openai")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "请先导入 OpenAI 官方订阅账号 JSON，再拉取模型".to_string())?;
        let plaintext = state
            .vault
            .decrypt(&credential.id, &credential.secret_envelope)
            .map_err(|error| error.to_string())?;
        let secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
            .map_err(|error| format!("凭据数据损坏：{error}"))?;
        let access_token = secret
            .access_token
            .ok_or_else(|| "OpenAI 官方账号缺少 access_token".to_string())?;
        let account_id = credential
            .account_id
            .ok_or_else(|| "OpenAI 官方账号缺少 account_id".to_string())?;
        (
            providers::discover_official_models(&access_token, &account_id)
                .await
                .map_err(|error| error.to_string())?,
            "https://chatgpt.com/backend-api/codex".to_string(),
        )
    } else {
        let models = providers::discover_models(
            &provider_id,
            &base_url,
            api_key.as_deref().map(String::as_str),
        )
        .await
        .map_err(|error| error.to_string())?;
        let normalized_base =
            providers::normalize_base_url(&base_url).map_err(|error| error.to_string())?;
        (models, normalized_base)
    };
    let records = models
        .iter()
        .map(|model| {
            providers::route_catalog_json(&provider_id, model)
                .map(|catalog_json| (model.id.clone(), model.display_name.clone(), catalog_json))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if provider_id != "openai" {
        if let Some(api_key) = api_key.as_deref().filter(|key| !key.trim().is_empty()) {
            let import_json = serde_json::json!({
                "provider": provider_id,
                "api_key": api_key.as_str(),
            })
            .to_string();
            if let Some(credential) = credentials::parse_json_import(&import_json)
                .map_err(|error| error.to_string())?
                .into_iter()
                .next()
            {
                let credential = credential.assign_provider(&provider_id);
                let id = Uuid::new_v4().to_string();
                let plaintext = Zeroizing::new(
                    providers::credential_secret_json(&credential)
                        .map_err(|error| error.to_string())?,
                );
                let envelope = state
                    .vault
                    .encrypt(&id, 1, plaintext.as_slice())
                    .map_err(|error| error.to_string())?;
                let envelope_json =
                    serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
                let inserted = state
                    .storage
                    .insert_credential(&credential, &id, &envelope_json)
                    .await
                    .map_err(|error| error.to_string())?;
                if inserted {
                    let pool_id = state
                        .storage
                        .ensure_default_pool(&provider_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    state
                        .storage
                        .add_pool_member(&pool_id, &id)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
        }
    }

    state
        .storage
        .replace_discovered_models(&provider_id, &normalized_base, &records)
        .await
        .map_err(|error| error.to_string())?;
    state.rebuild_runtime().await?;
    list_model_routes(state).await
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
    state.rebuild_runtime().await?;
    list_model_routes(state).await
}

#[tauri::command]
async fn import_credentials_json(
    state: State<'_, AppState>,
    provider_id: String,
    input: String,
) -> Result<Vec<CredentialSummary>, String> {
    let credentials = credentials::parse_json_import(&input).map_err(|error| error.to_string())?;
    for credential in credentials {
        let credential = credential.assign_provider(&provider_id);
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
                .ensure_default_pool(&provider_id)
                .await
                .map_err(|error| error.to_string())?;
            state
                .storage
                .add_pool_member(&pool_id, &id)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    list_credentials(state, Some(provider_id)).await
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
    let base_url = state
        .storage
        .provider_base_url(&credential.provider_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "供应商尚未配置 Base URL".to_string())?;
    let plaintext = state
        .vault
        .decrypt(&credential.id, &credential.secret_envelope)
        .map_err(|error| error.to_string())?;
    let secret = SecretMaterial::from_json_bytes(plaintext.as_slice())
        .map_err(|error| format!("凭据数据损坏：{error}"))?;
    let result =
        providers::test_credential(&credential.provider_id, &base_url, &model_id, &secret).await;
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
        .icon(tray_icon)
        .icon_as_template(true)
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
                    match state.storage.list_credentials(Some("openai")).await {
                        Ok(accounts) => {
                            for account in accounts {
                                if let Err(error) = refresh_quota_for_state(&state, &account.id).await {
                                    tracing::warn!(account = %account.fingerprint_prefix, %error, "failed to refresh OpenAI quota");
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

    tauri::Builder::default()
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
            preview_codex_apply,
            apply_codex_config,
            restore_previous_codex_config,
            list_model_routes,
            discover_provider_models,
            set_model_enabled,
            import_credentials_json,
            list_credentials,
            test_account,
            refresh_openai_quota,
            get_cached_openai_quota,
            consume_openai_reset_credit,
            create_account_pool,
            list_account_pools,
            add_account_to_pool,
            remove_account_from_pool,
            list_pool_member_ids,
            restart_proxy,
            proxy_secret_available,
            inspect_credential_json,
            keychain_ready,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Codex Spur");
}
