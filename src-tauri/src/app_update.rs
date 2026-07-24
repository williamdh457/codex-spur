//! Check GitHub Releases and apply an in-app macOS update (unsigned DMG).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const GITHUB_OWNER: &str = "williamdh457";
const GITHUB_REPO: &str = "codex-spur";
const APP_BUNDLE_NAME: &str = "Codex Spur.app";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_name: String,
    pub release_url: String,
    pub release_notes: Option<String>,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
    pub published_at: Option<String>,
    pub platform: String,
    pub architecture: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInstallResult {
    pub message: String,
    pub install_path: String,
    pub version: String,
    pub will_relaunch: bool,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

pub fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        "other"
    }
}

pub fn host_architecture() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "unknown"
    }
}

/// Compare dotted numeric versions (`1.2.3`, optional leading `v`).
/// Returns `Ordering` of `left` vs `right`.
pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let lp = version_parts(left);
    let rp = version_parts(right);
    let len = lp.len().max(rp.len());
    for i in 0..len {
        let l = lp.get(i).copied().unwrap_or(0);
        let r = rp.get(i).copied().unwrap_or(0);
        match l.cmp(&r) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn version_parts(raw: &str) -> Vec<u64> {
    let trimmed = raw.trim().trim_start_matches('v').trim_start_matches('V');
    let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
    core.split('.')
        .filter_map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse().ok()
            }
        })
        .collect()
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("Codex-Spur/{}", current_version()))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))
}

fn pick_asset<'a>(assets: &'a [GhAsset], platform: &str, arch: &str) -> Option<&'a GhAsset> {
    let name_ok = |name: &str| {
        let lower = name.to_ascii_lowercase();
        match platform {
            "macOS" => {
                let is_dmg = lower.ends_with(".dmg");
                let arch_ok = match arch {
                    "aarch64" => {
                        lower.contains("aarch64")
                            || lower.contains("arm64")
                            || lower.contains("apple-silicon")
                    }
                    "x86_64" => {
                        lower.contains("x64")
                            || lower.contains("x86_64")
                            || lower.contains("amd64")
                            || lower.contains("intel")
                    }
                    _ => false,
                };
                is_dmg && arch_ok
            }
            "Windows" => {
                let is_installer =
                    lower.ends_with(".exe") || lower.ends_with(".msi") || lower.ends_with(".nsis.zip");
                let arch_ok = lower.contains("x64")
                    || lower.contains("x86_64")
                    || lower.contains("amd64")
                    || !lower.contains("arm");
                is_installer && arch_ok
            }
            _ => false,
        }
    };

    assets.iter().find(|a| name_ok(&a.name)).or_else(|| {
        // Fallback: any DMG / exe on the expected platform.
        assets.iter().find(|a| {
            let lower = a.name.to_ascii_lowercase();
            match platform {
                "macOS" => lower.ends_with(".dmg"),
                "Windows" => lower.ends_with(".exe") || lower.ends_with(".msi"),
                _ => false,
            }
        })
    })
}

pub async fn check_for_app_update() -> Result<AppUpdateInfo, String> {
    let current = current_version();
    let platform = host_platform().to_string();
    let architecture = host_architecture().to_string();
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest"
    );
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("检查更新失败（网络）：{e}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(AppUpdateInfo {
            current_version: current.clone(),
            latest_version: current,
            update_available: false,
            release_name: "无可用 Release".into(),
            release_url: format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases"),
            release_notes: Some("仓库还没有公开的最新 Release。".into()),
            asset_name: None,
            asset_url: None,
            asset_size: None,
            published_at: None,
            platform,
            architecture,
        });
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(format!("GitHub API 返回 {status}：{snippet}"));
    }

    let release: GhRelease = response
        .json()
        .await
        .map_err(|e| format!("解析 GitHub Release 失败：{e}"))?;

    let latest = release.tag_name.trim().trim_start_matches('v').to_string();
    let update_available = compare_versions(&latest, &current) == std::cmp::Ordering::Greater;
    let asset = pick_asset(&release.assets, &platform, &architecture);

    Ok(AppUpdateInfo {
        current_version: current,
        latest_version: latest,
        update_available,
        release_name: release
            .name
            .unwrap_or_else(|| format!("v{}", release.tag_name.trim_start_matches('v'))),
        release_url: release.html_url,
        release_notes: release.body,
        asset_name: asset.map(|a| a.name.clone()),
        asset_url: asset.map(|a| a.browser_download_url.clone()),
        asset_size: asset.map(|a| a.size),
        published_at: release.published_at,
        platform,
        architecture,
    })
}

/// Download the latest macOS DMG and replace the installed app, then relaunch.
pub async fn install_app_update(app: AppHandle) -> Result<AppUpdateInstallResult, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        return Err("一键更新目前仅支持 macOS。Windows 请从 GitHub Release 下载安装包。".into());
    }

    #[cfg(target_os = "macos")]
    {
        install_app_update_macos(app).await
    }
}

#[cfg(target_os = "macos")]
async fn install_app_update_macos(app: AppHandle) -> Result<AppUpdateInstallResult, String> {
    let info = check_for_app_update().await?;
    if !info.update_available {
        return Err(format!(
            "当前已是最新版本 v{}，无需更新。",
            info.current_version
        ));
    }
    let asset_url = info
        .asset_url
        .clone()
        .ok_or_else(|| {
            format!(
                "最新版本 v{} 没有适用于 {} {} 的安装包。请打开 Release 页面手动下载，或从源码构建。",
                info.latest_version, info.platform, info.architecture
            )
        })?;
    let asset_name = info
        .asset_name
        .clone()
        .unwrap_or_else(|| "Codex.Spur.update.dmg".into());

    if !asset_name.to_ascii_lowercase().ends_with(".dmg") {
        return Err(format!(
            "最新资产不是 DMG（{asset_name}）。请打开 Release 手动安装。"
        ));
    }

    let install_path = resolve_install_path(&app)?;
    let staging_root = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("无法定位缓存目录：{e}"))?
        .join("updates");
    fs::create_dir_all(&staging_root).map_err(|e| format!("创建更新缓存目录失败：{e}"))?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let work_dir = staging_root.join(format!("v{}-{stamp}", info.latest_version));
    if work_dir.exists() {
        let _ = fs::remove_dir_all(&work_dir);
    }
    fs::create_dir_all(&work_dir).map_err(|e| format!("创建更新工作目录失败：{e}"))?;

    let dmg_path = work_dir.join(&asset_name);
    download_file(&asset_url, &dmg_path).await?;

    let mount_point = work_dir.join("mnt");
    fs::create_dir_all(&mount_point).map_err(|e| format!("创建挂载点失败：{e}"))?;
    attach_dmg(&dmg_path, &mount_point)?;

    let source_app = find_app_bundle(&mount_point).map_err(|err| {
        let _ = detach_dmg(&mount_point);
        err
    })?;

    let staged_app = work_dir.join(APP_BUNDLE_NAME);
    if staged_app.exists() {
        let _ = fs::remove_dir_all(&staged_app);
    }
    ditto_copy(&source_app, &staged_app).map_err(|err| {
        let _ = detach_dmg(&mount_point);
        err
    })?;
    let _ = detach_dmg(&mount_point);

    // Clear quarantine on the staged bundle before swap.
    clear_quarantine(&staged_app);

    let helper = work_dir.join("apply-update.sh");
    write_apply_script(&helper, &staged_app, &install_path, &work_dir)?;

    // Detached helper: waits for this process to exit, swaps the app, relaunches.
    Command::new("/bin/bash")
        .arg(&helper)
        .current_dir(&work_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("无法启动更新助手：{e}"))?;

    // Give the helper a moment to start, then exit so the swap can proceed.
    let app_for_exit = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        app_for_exit.exit(0);
    });

    Ok(AppUpdateInstallResult {
        message: format!(
            "已下载 v{}，正在替换应用并重启…",
            info.latest_version
        ),
        install_path: install_path.display().to_string(),
        version: info.latest_version,
        will_relaunch: true,
    })
}

fn resolve_install_path(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bundle) = find_enclosing_app_bundle(&exe) {
            return Ok(bundle);
        }
    }
    // Prefer resource path if packaged.
    if let Ok(resource) = app.path().resource_dir() {
        if let Some(bundle) = find_enclosing_app_bundle(&resource) {
            return Ok(bundle);
        }
    }
    Ok(PathBuf::from("/Applications").join(APP_BUNDLE_NAME))
}

fn find_enclosing_app_bundle(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".app"))
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

async fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let client = http_client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载更新失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("下载更新失败：HTTP {}", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("读取更新内容失败：{e}"))?;
    if bytes.len() < 1_024 {
        return Err("下载的文件过小，可能不是有效安装包。".into());
    }
    let mut file = fs::File::create(dest).map_err(|e| format!("写入更新文件失败：{e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("写入更新文件失败：{e}"))?;
    file.sync_all()
        .map_err(|e| format!("同步更新文件失败：{e}"))?;
    Ok(())
}

fn attach_dmg(dmg: &Path, mount_point: &Path) -> Result<(), String> {
    let output = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noverify",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(mount_point)
        .arg(dmg)
        .output()
        .map_err(|e| format!("挂载 DMG 失败：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("挂载 DMG 失败：{stderr}"));
    }
    Ok(())
}

fn detach_dmg(mount_point: &Path) -> Result<(), String> {
    let output = Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(mount_point)
        .output()
        .map_err(|e| format!("卸载 DMG 失败：{e}"))?;
    if !output.status.success() {
        // Force detach once.
        let _ = Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(mount_point)
            .output();
    }
    Ok(())
}

fn find_app_bundle(mount_point: &Path) -> Result<PathBuf, String> {
    let preferred = mount_point.join(APP_BUNDLE_NAME);
    if preferred.is_dir() {
        return Ok(preferred);
    }
    let entries = fs::read_dir(mount_point).map_err(|e| format!("读取 DMG 内容失败：{e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".app"))
            && path.is_dir()
        {
            return Ok(path);
        }
    }
    Err("DMG 内未找到 .app 应用包。".into())
}

fn ditto_copy(src: &Path, dest: &Path) -> Result<(), String> {
    let output = Command::new("ditto")
        .arg(src)
        .arg(dest)
        .output()
        .map_err(|e| format!("复制应用失败：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("复制应用失败：{stderr}"));
    }
    Ok(())
}

fn clear_quarantine(path: &Path) {
    let _ = Command::new("xattr").args(["-cr"]).arg(path).output();
}

fn write_apply_script(
    helper: &Path,
    staged_app: &Path,
    install_path: &Path,
    work_dir: &Path,
) -> Result<(), String> {
    let parent = install_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/Applications"));
    let backup = parent.join("Codex Spur.app.previous");
    let pid = std::process::id();

    // Shell script is local-only; paths are absolute from our process.
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail
STAGED="{staged}"
DEST="{dest}"
BACKUP="{backup}"
WORK="{work}"
PID="{pid}"

# Wait for the running app process to exit (max ~60s).
for _ in $(seq 1 120); do
  if ! kill -0 "$PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
sleep 0.5

if [ ! -d "$STAGED" ]; then
  echo "staged app missing" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
rm -rf "$BACKUP"
if [ -d "$DEST" ] || [ -e "$DEST" ]; then
  mv "$DEST" "$BACKUP" || rm -rf "$DEST"
fi
ditto "$STAGED" "$DEST"
xattr -cr "$DEST" || true
open "$DEST" || true

# Best-effort cleanup of staging (keep backup for recovery).
rm -rf "$WORK" || true
"#,
        staged = shell_escape(&staged_app.display().to_string()),
        dest = shell_escape(&install_path.display().to_string()),
        backup = shell_escape(&backup.display().to_string()),
        work = shell_escape(&work_dir.display().to_string()),
        pid = pid,
    );

    fs::write(helper, script).map_err(|e| format!("写入更新脚本失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(helper)
            .map_err(|e| format!("读取脚本权限失败：{e}"))?
            .permissions();
        perms.set_mode(0o700);
        fs::set_permissions(helper, perms).map_err(|e| format!("设置脚本权限失败：{e}"))?;
    }
    Ok(())
}

fn shell_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_with_v_prefix() {
        assert_eq!(
            compare_versions("0.1.6", "0.1.5"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("v0.1.5", "0.1.5"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_versions("0.1.4", "0.1.5"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_versions("0.2.0", "0.1.99"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn picks_aarch64_dmg() {
        let assets = vec![
            GhAsset {
                name: "Codex.Spur_0.1.6_x64-setup.exe".into(),
                browser_download_url: "https://example.com/win".into(),
                size: 1,
            },
            GhAsset {
                name: "Codex.Spur_0.1.6_aarch64.dmg".into(),
                browser_download_url: "https://example.com/mac".into(),
                size: 2,
            },
        ];
        let picked = pick_asset(&assets, "macOS", "aarch64").unwrap();
        assert!(picked.name.contains("aarch64"));
    }
}
