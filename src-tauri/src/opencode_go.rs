use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use zeroize::Zeroizing;

pub const DEFAULT_BASE_URL: &str = "https://opencode.ai/zen/go/v1";

#[derive(Debug, Deserialize)]
struct AuthFile {
    #[serde(rename = "opencode-go")]
    opencode_go: Option<AuthEntry>,
}

#[derive(Debug, Deserialize)]
struct AuthEntry {
    #[serde(rename = "type")]
    kind: String,
    key: Option<String>,
}

pub fn auth_path() -> anyhow::Result<PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(data_home).join("opencode/auth.json"));
    }
    let home = directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| anyhow!("无法确定用户目录"))?;
    Ok(home.join(".local/share/opencode/auth.json"))
}

pub async fn read_api_key(path: &Path) -> anyhow::Result<Zeroizing<String>> {
    let raw = tokio::fs::read(path)
        .await
        .with_context(|| format!("无法读取 OpenCode 凭据文件：{}", path.display()))?;
    parse_api_key(&raw)
}

fn parse_api_key(raw: &[u8]) -> anyhow::Result<Zeroizing<String>> {
    let parsed: AuthFile =
        serde_json::from_slice(raw).context("OpenCode auth.json 不是有效 JSON")?;
    let entry = parsed
        .opencode_go
        .ok_or_else(|| anyhow!("未找到 opencode-go 凭据，请先在 OpenCode 登录 OpenCode Go"))?;
    if entry.kind.trim() != "api" {
        return Err(anyhow!("opencode-go 凭据类型不是 api"));
    }
    let key = entry.key.unwrap_or_default();
    let key = key.trim();
    if key.is_empty() {
        return Err(anyhow!("opencode-go API Key 为空"));
    }
    Ok(Zeroizing::new(key.to_string()))
}

pub fn path_label(path: &Path) -> String {
    if let Some(home) = directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        if let Ok(relative) = path.strip_prefix(home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_opencode_go_api_key() {
        let key = parse_api_key(
            br#"{"opencode":{"type":"api","key":"zen"},"opencode-go":{"type":"api","key":"go-secret"}}"#,
        )
        .expect("go credential");
        assert_eq!(key.as_str(), "go-secret");
    }

    #[test]
    fn rejects_zen_only_and_invalid_go_entries() {
        assert!(parse_api_key(br#"{"opencode":{"type":"api","key":"zen"}}"#)
            .unwrap_err()
            .to_string()
            .contains("opencode-go"));
        assert!(
            parse_api_key(br#"{"opencode-go":{"type":"oauth","key":"x"}}"#)
                .unwrap_err()
                .to_string()
                .contains("不是 api")
        );
        assert!(
            parse_api_key(br#"{"opencode-go":{"type":"api","key":"  "}}"#)
                .unwrap_err()
                .to_string()
                .contains("为空")
        );
    }
}
