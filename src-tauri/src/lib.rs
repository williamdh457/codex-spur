#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod catalog;
pub mod codex_config;
mod content_encoding;
mod credentials;
mod domain;
mod openai_oauth;
pub mod providers;
mod proxy;
mod quota;
mod scheduler;
pub mod storage;
pub mod vault;

use std::sync::Arc;

use credentials::{CredentialImportSummary, SecretMaterial};
use domain::{
    AccountPoolSummary, AppSnapshot, ApplyPreview, CodexApplyOutcome, CodexBindingStatus,
    CredentialSummary, ModelRouteSummary, OpenAiQuotaSnapshot, PoolMemberDetail, ProviderRouting,
    ProviderSummary, ProxyRequestEvent, ProxyStatus,
};
use scheduler::PoolSchedulerConfig;
use tauri::{
    menu::{MenuBuilder, MenuItem},
    tray::TrayIconBuilder,
    Manager, RunEvent, State, WindowEvent,
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
    openai_oauth: openai_oauth::OpenAiOAuthManager,
}

impl AppState {
    async fn bootstrap(data_dir: std::path::PathBuf) -> anyhow::Result<Self> {
        let storage = Arc::new(storage::Storage::open(&data_dir).await?);
        let vault = Arc::new(vault::SecretVault::load_or_create(&data_dir)?);
        // Scrub legacy camelCase / GPT-tool ads from SQLite so every subsequent
        // apply/rebuild starts from Codex-safe snake_case rows.
        match storage.heal_all_route_catalogs().await {
            Ok(0) => {}
            Ok(n) => tracing::info!(healed_routes = n, "已将 model_routes.catalog_json heal 为 snake_case"),
            Err(error) => tracing::warn!(%error, "启动时 heal catalog_json 失败，将继续用运行时 heal"),
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
        let live = codex_config::inspect_live_binding();
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
        };
        Ok(Self {
            snapshot: RwLock::new(snapshot),
            catalog,
            routes,
            storage,
            proxy: RwLock::new(proxy),
            vault,
            openai_oauth: openai_oauth::OpenAiOAuthManager::new(),
        })
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
        // Refresh binding + attention from live ~/.codex (not isolated CODEX_HOME).
        let live = codex_config::inspect_live_binding();
        snapshot.binding.state = live.state;
        snapshot.binding.codex_home = live.codex_home.display().to_string();
        snapshot.binding.provider_id = live.provider_id;
        snapshot.binding.catalog_path = live.catalog_path.display().to_string();
        snapshot.attention_items = live.attention;
        if published_models == 0 {
            snapshot
                .attention_items
                .push("添加供应商并拉取模型后，才能应用到 Codex。".into());
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
    state
        .storage
        .usage_snapshot()
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
        snapshot.binding.state = live.state;
        snapshot.binding.codex_home = live.codex_home.display().to_string();
        snapshot.binding.provider_id = live.provider_id.clone();
        snapshot.binding.catalog_path = live.catalog_path.display().to_string();
        snapshot.published_models = result.model_count;
        snapshot.attention_items = live.attention;
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
        let _ = state
            .storage
            .set_active_pool(provider_id, &pool_id)
            .await;
    }
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
    state.rebuild_runtime().await?;
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
    let (models, normalized_base) = if official_openai {
        let credential = state
            .storage
            .first_healthy_credential(&provider_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "请先通过「OpenAI · 导入账号池」导入官方订阅账号，再拉取模型".to_string()
            })?;
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
                "缺少 API Key。请到「API 配置」填写，或先在「导入 JSON」导入账号后再拉取。"
                    .into(),
            );
        }
        let models = providers::discover_models(
            &provider.kind,
            &effective_base,
            bearer.as_deref(),
        )
        .await
        .map_err(|error| error.to_string())?;
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

#[tauri::command]
async fn open_external_url(url: String) -> Result<(), String> {
    open_url_in_browser(&url)
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

async fn store_oauth_tokens_credential(
    state: &AppState,
    provider_id: &str,
    tokens: &openai_oauth::DeviceLoginTokens,
) -> Result<(), String> {
    let mut import = serde_json::json!({
        "provider": provider_id,
        "access_token": tokens.access_token,
        "account_id": tokens.account_id,
    });
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        import["refresh_token"] = serde_json::Value::String(refresh.to_string());
    }
    if let Some(id_token) = tokens.id_token.as_deref() {
        import["id_token"] = serde_json::Value::String(id_token.to_string());
    }
    if let Some(email) = tokens.email.as_deref() {
        import["email"] = serde_json::Value::String(email.to_string());
    }
    if let Some(expires_at) = tokens.expires_at {
        import["expires_at"] = serde_json::Value::Number(expires_at.into());
    }
    let credentials = credentials::parse_json_import(&import.to_string())
        .map_err(|error| error.to_string())?;
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

/// Create an OpenAI instance from completed device-login tokens and fetch models.
#[tauri::command]
async fn complete_openai_device_login(
    state: State<'_, AppState>,
    name: Option<String>,
    tokens: openai_oauth::DeviceLoginTokens,
) -> Result<OpenAiLoginComplete, String> {
    let id = state
        .storage
        .create_provider_instance("openai", name.as_deref())
        .await
        .map_err(|error| error.to_string())?;
    store_oauth_tokens_credential(&state, &id, &tokens).await?;
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
                publish_discovered_models(&state, &provider, &models, official_base).await?;
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
    state.rebuild_runtime().await?;
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
            let msg = "模型选择已变更：请再次点击 Review & Apply，然后 Cmd+Q 完全退出 ChatGPT 再打开，右下角才会刷新。";
            if !snapshot.attention_items.iter().any(|item| item == msg) {
                snapshot.attention_items.push(msg.into());
            }
        }
    }
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
async fn update_pool_member(
    state: State<'_, AppState>,
    pool_id: String,
    credential_id: String,
    weight: i64,
    priority: i64,
    enabled: bool,
    concurrency_limit: i64,
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
        .set_provider_routing(
            &provider_id,
            &routing_mode,
            fixed_credential_id.as_deref(),
        )
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
            preview_codex_apply,
            apply_codex_config,
            restore_previous_codex_config,
            list_model_routes,
            discover_provider_models,
            import_provider_config_json,
            create_provider_instance,
            delete_provider_instance,
            rename_provider_instance,
            start_openai_device_login,
            poll_openai_device_login,
            cancel_openai_device_login,
            complete_openai_device_login,
            open_external_url,
            set_active_pool,
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
