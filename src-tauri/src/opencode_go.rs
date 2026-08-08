use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::Deserialize;
use zeroize::Zeroizing;

pub const DEFAULT_BASE_URL: &str = "https://opencode.ai/zen/go/v1";

/// OpenCode Go is a tri-protocol gateway on one base URL.
/// Route by upstream model id (official Go docs), not by provider protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoWireProtocol {
    /// Most models: Grok / GLM / Kimi / DeepSeek / MiMo / Hy3.
    ChatCompletions,
    /// `gpt-5.6-luna` only (OpenAI Responses API).
    Responses,
    /// MiniMax + Qwen families (Anthropic Messages API).
    AnthropicMessages,
}

/// Pick the upstream wire for an OpenCode Go model id.
///
/// Bare catalog ids (`kimi-k3`) and prefixed forms (`opencode-go/kimi-k3`) both work.
/// Default is Chat Completions — including `deepseek-v4-flash` on Go (unlike Spur's
/// native DeepSeek kind, which uses Responses for V4 Flash).
pub fn wire_protocol_for_model(upstream_model: &str) -> GoWireProtocol {
    let id = upstream_model.trim().to_ascii_lowercase();
    let tail = id.rsplit(['/', ':']).next().unwrap_or(&id);

    // Official Go docs: only gpt-5.6-luna uses /responses.
    if matches!(tail, "gpt-5.6-luna" | "gpt-5-6-luna")
        || tail.starts_with("gpt-5.6-luna")
        || tail.starts_with("gpt-5-6-luna")
    {
        return GoWireProtocol::Responses;
    }

    // Official Go docs: MiniMax + Qwen → Anthropic /messages.
    if tail.starts_with("minimax") || tail.starts_with("qwen") {
        return GoWireProtocol::AnthropicMessages;
    }

    GoWireProtocol::ChatCompletions
}

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

    #[test]
    fn wire_protocol_routes_by_model_family() {
        assert_eq!(
            wire_protocol_for_model("gpt-5.6-luna"),
            GoWireProtocol::Responses
        );
        assert_eq!(
            wire_protocol_for_model("opencode-go/gpt-5.6-luna"),
            GoWireProtocol::Responses
        );
        assert_eq!(
            wire_protocol_for_model("gpt-5-6-luna"),
            GoWireProtocol::Responses
        );

        for id in [
            "minimax-m3",
            "minimax-m2.7",
            "minimax-m2.5",
            "qwen3.8-max",
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.6-plus",
            "qwen3.5-plus",
            "OpenCode-Go/Qwen3.7-Max",
        ] {
            assert_eq!(
                wire_protocol_for_model(id),
                GoWireProtocol::AnthropicMessages,
                "{id}"
            );
        }

        for id in [
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "grok-4.5",
            "glm-5.2",
            "kimi-k3",
            "kimi-k2.7-code",
            "mimo-v2.5",
            "hy3",
            "hy3-preview",
        ] {
            assert_eq!(
                wire_protocol_for_model(id),
                GoWireProtocol::ChatCompletions,
                "{id}"
            );
        }
    }
}
