//! One-shot: rebuild catalog from Spur SQLite and publish into ~/.codex.
//! Usage: cargo run --manifest-path src-tauri/Cargo.toml --bin codex-spur-publish

use std::path::PathBuf;

use codex_select_lib::catalog;
use codex_select_lib::codex_config;
use codex_select_lib::storage::Storage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var_os("CODEX_SPUR_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_spur_data_dir);

    println!("data_dir={}", data_dir.display());
    let storage = Storage::open(&data_dir).await?;
    match storage.heal_all_route_catalogs().await {
        Ok(n) if n > 0 => println!("healed_sqlite_routes={n}"),
        Ok(_) => {}
        Err(error) => eprintln!("warn: heal sqlite failed: {error}"),
    }
    let routes = storage.list_routes(true).await?;
    if routes.is_empty() {
        anyhow::bail!("没有已启用的模型路由。请先在 Codex Spur 里勾选模型。");
    }

    let (catalog, _targets) = catalog::build_from_routes(&routes)?;
    println!("enabled_models={}", catalog.models.len());
    for model in &catalog.models {
        println!("  - {} | {}", model.slug, model.display_name);
    }

    // Prefer the persisted Spur proxy bearer so restarts keep Codex config valid.
    let home = codex_config::publish_codex_home();
    let config_path = codex_config::config_path_for(&home);
    let bearer = read_persisted_proxy_bearer(&data_dir)
        .or_else(|| read_existing_bearer(&config_path))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // Persist if we invented one so the next Spur start matches.
    let _ = std::fs::create_dir_all(&data_dir);
    let token_path = data_dir.join("proxy_bearer_token");
    if !token_path.exists() {
        let _ = std::fs::write(&token_path, format!("{bearer}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    let base_url = "http://127.0.0.1:17861/v1";

    let result = codex_config::apply(base_url, &bearer, &catalog)?;
    let live = codex_config::inspect_live_binding();
    let visibility = codex_config::inspect_desktop_visibility(None, Some(base_url));

    println!("config={}", result.config_path.display());
    println!("catalog={}", result.catalog_path.display());
    println!("selected={:?}", result.selected_model);
    println!("live_state={}", live.state);
    println!(
        "desktop_visibility={} ready={}",
        visibility.status_label, visibility.ready
    );
    for check in &visibility.checks {
        println!(
            "  [{}] {} — {}",
            if check.ok { "ok" } else { "!!" },
            check.label,
            check.detail
        );
    }
    for warning in &result.warnings {
        println!("warning: {warning}");
    }
    if live.state != "applied" {
        anyhow::bail!("publish 后 live state 仍是 {}，未绑定 codex_select", live.state);
    }
    if !visibility.ready {
        eprintln!(
            "warn: Desktop 可见性未就绪（{}）。Kimi/DeepSeek 可能仍被 GUI 隐藏。",
            visibility.status_label
        );
    }
    println!("OK published {} models to {}", result.model_count, home.display());
    Ok(())
}

fn read_persisted_proxy_bearer(data_dir: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join("proxy_bearer_token")).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_existing_bearer(config_path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(config_path).ok()?;
    // Prefer codex_select token under [model_providers.codex_select]
    let mut in_select = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_select = trimmed == "[model_providers.codex_select]";
            continue;
        }
        if in_select && trimmed.starts_with("experimental_bearer_token") {
            if let Some(rest) = trimmed.split_once('=') {
                let value = rest.1.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn default_spur_data_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "codexspur", "desktop") {
        return dirs.data_dir().to_path_buf();
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library/Application Support/com.codexspur.desktop");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("com.codexspur.desktop");
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Roaming")
                .join("com.codexspur.desktop");
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/share/com.codexspur.desktop");
        }
    }
    PathBuf::from("com.codexspur.desktop")
}
