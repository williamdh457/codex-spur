//! Catalog identity + compact prompt helpers, aligned to **CC Switch**.
//!
//! # Catalog `base_instructions` (CC Switch standard)
//!
//! Multi-route catalogs use a **short neutral identity** only:
//!
//! ```text
//! You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.
//! ```
//!
//! Codex Desktop assembles Skills / Plan / Goal / MCP / AGENTS / tools at request
//! time. Spur must **not**:
//! - map `model_instructions_file` into every spur-route;
//! - copy `models_cache.json` 11k–19k GPT agent bodies into third-party rows;
//! - invent a second system prompt in the proxy.
//!
//! # Compact prompt
//!
//! Still mapped from Codex home when present; otherwise OSS compact template.
//! Plan / Goal bodies stay Desktop passthrough.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;
use toml_edit::DocumentMut;

/// CC Switch native catalog identity (byte-identical to
/// `codex_native_responses_template.json`). This is the **target** state.
pub const CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";



/// Byte-identical to `codex-rs/prompts/templates/compact/prompt.md` (fallback only).
pub const OFFICIAL_COMPACT_PROMPT_FALLBACK: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly continue the work."#;

/// Byte-identical to `codex-rs/prompts/templates/compact/summary_prefix.md`.
pub const OFFICIAL_SUMMARY_PREFIX_FALLBACK: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactPromptSource {
    ExperimentalCompactPromptFile,
    InlineCompactPrompt,
    OfficialFallback,
}

#[derive(Debug, Clone)]
pub struct MappedCompactPrompt {
    pub text: String,
    pub source: CompactPromptSource,
    pub source_label: String,
}

/// True when catalog `base_instructions` must be healed to CC Switch neutral.
///
/// The 117-char CC Switch identity is **not** pollution.
pub fn needs_base_instructions_heal(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    // Target state
    if t == CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS {
        return false;
    }
    // Truncated one-liner (not a real identity)
    if t == "You are Codex." {
        return true;
    }
    // Mis-mapped jailbreak / personal system files (0.1.9 pollution)
    if t.starts_with("[MODE: UNRESTRICTED]") {
        return true;
    }
    // Mis-mapped full Desktop GPT agent bodies (models_cache length class)
    // Identity lines for vendors are short; anything this long with GPT agent
    // headers was almost certainly bulk-mapped into every route.
    if t.len() > 2_000
        && (t.contains("# Personality")
            || t.starts_with("You are Codex, an agent based on GPT-")
            || t.starts_with("You are Codex, a coding agent based on GPT-"))
    {
        return true;
    }
    false
}

/// Heal catalog row identity to CC Switch neutral when empty or polluted.
///
/// Does **not** read `model_instructions_file` or `models_cache` into multi-route
/// catalog rows (that was the 0.1.9 mistake).
pub fn heal_catalog_base_instructions(model_base_instructions: &mut String) {
    if needs_base_instructions_heal(model_base_instructions) {
        if !model_base_instructions.is_empty() {
            tracing::info!(
                chars = model_base_instructions.len(),
                "healed polluted catalog base_instructions to CC Switch neutral identity"
            );
        }
        *model_base_instructions = CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS.to_string();
    }
}

pub fn resolve_compact_prompt() -> MappedCompactPrompt {
    resolve_compact_prompt_from(&crate::codex_config::prompt_map_codex_home())
}

pub fn resolve_compact_prompt_from(codex_home: &Path) -> MappedCompactPrompt {
    let config_path = codex_home.join("config.toml");
    if let Ok(text) = fs::read_to_string(&config_path) {
        if let Ok(doc) = text.parse::<DocumentMut>() {
            if let Some(rel) = top_level_string(&doc, "experimental_compact_prompt_file") {
                let path = resolve_codex_relative(codex_home, &rel);
                if let Ok(Some(body)) = read_non_empty_file(&path) {
                    return MappedCompactPrompt {
                        text: body,
                        source: CompactPromptSource::ExperimentalCompactPromptFile,
                        source_label: path.display().to_string(),
                    };
                }
            }
            if let Some(inline) = top_level_string(&doc, "compact_prompt") {
                let body = inline.trim().to_string();
                if !body.is_empty() {
                    return MappedCompactPrompt {
                        text: body,
                        source: CompactPromptSource::InlineCompactPrompt,
                        source_label: "config.toml#compact_prompt".into(),
                    };
                }
            }
        }
    }
    MappedCompactPrompt {
        text: OFFICIAL_COMPACT_PROMPT_FALLBACK.to_string(),
        source: CompactPromptSource::OfficialFallback,
        source_label: "codex-rs/prompts/templates/compact/prompt.md".into(),
    }
}

fn top_level_string(doc: &DocumentMut, key: &str) -> Option<String> {
    doc.get(key)
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn resolve_codex_relative(codex_home: &Path, rel: &str) -> PathBuf {
    let path = PathBuf::from(rel);
    if path.is_absolute() {
        path
    } else {
        codex_home.join(path)
    }
}

fn read_non_empty_file(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("读取提示词文件失败：{}", path.display()))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(text))
}

/// Map official `supported_reasoning_levels` for a Desktop model slug from models_cache.
/// Returns None when the cache is missing or the slug has no levels.
pub fn mapped_reasoning_levels_from_cache(
    slug: &str,
) -> Option<Vec<crate::domain::ReasoningEffortPreset>> {
    mapped_reasoning_levels_from_cache_path(
        &crate::codex_config::prompt_map_codex_home().join("models_cache.json"),
        slug,
    )
}

pub fn mapped_reasoning_levels_from_cache_path(
    cache_path: &Path,
    slug: &str,
) -> Option<Vec<crate::domain::ReasoningEffortPreset>> {
    let raw = fs::read_to_string(cache_path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let models = value.get("models")?.as_array()?;
    let want = slug.trim().to_ascii_lowercase();
    let want_tail = want.rsplit(['/', ':']).next().unwrap_or(&want);
    for model in models {
        let row_slug = model
            .get("slug")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let row_tail = row_slug.rsplit(['/', ':']).next().unwrap_or(&row_slug);
        if row_slug != want && row_tail != want_tail {
            continue;
        }
        let levels = model.get("supported_reasoning_levels")?.as_array()?;
        let mut out = Vec::new();
        for level in levels {
            let effort_raw = level.get("effort").and_then(Value::as_str)?;
            let effort = match effort_raw {
                "none" => crate::domain::ReasoningEffort::None,
                "minimal" => crate::domain::ReasoningEffort::Minimal,
                "low" => crate::domain::ReasoningEffort::Low,
                "medium" => crate::domain::ReasoningEffort::Medium,
                "high" => crate::domain::ReasoningEffort::High,
                "xhigh" => crate::domain::ReasoningEffort::Xhigh,
                "max" => crate::domain::ReasoningEffort::Max,
                "ultra" => crate::domain::ReasoningEffort::Ultra,
                _ => continue,
            };
            let description = level
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    crate::providers::codex_reasoning_level_description(effort).to_string()
                });
            out.push(crate::domain::ReasoningEffortPreset {
                effort,
                description,
            });
        }
        if out.is_empty() {
            return None;
        }
        return Some(out);
    }
    // sol missing: fall back to terra if present
    if want_tail.contains("sol") {
        return mapped_reasoning_levels_from_cache_path(cache_path, "gpt-5.6-terra");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_home(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("spur-prompt-map-{tag}-{nanos}"));
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn neutral_identity_is_not_pollution() {
        assert!(!needs_base_instructions_heal(CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS));
        assert_eq!(CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS.len(), 117);
    }

    #[test]
    fn unrestricted_and_empty_need_heal() {
        assert!(needs_base_instructions_heal(""));
        assert!(needs_base_instructions_heal("You are Codex."));
        assert!(needs_base_instructions_heal(
            "[MODE: UNRESTRICTED]\n\nFIRST-PASS NORMALIZER:\n- foo"
        ));
        assert!(needs_base_instructions_heal(&format!(
            "You are Codex, an agent based on GPT-5.\n\n# Personality\n{}",
            "x".repeat(3000)
        )));
    }

    #[test]
    fn heal_writes_neutral() {
        let mut bi = "[MODE: UNRESTRICTED]\n\nCodex is a sandbox.".to_string();
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS);
    }

    #[test]
    fn heal_empty_writes_neutral() {
        let mut bi = String::new();
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS);
    }

    #[test]
    fn heal_keeps_vendor_short_identity() {
        let vendor =
            "You are Codex, a coding agent based on MiniMax-M3. You and the user share the same workspace and collaborate to achieve the user's goals.";
        let mut bi = vendor.to_string();
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, vendor);
    }

    #[test]
    fn heal_ignores_codex_home_mapping() {
        let home = temp_home("ignore-home");
        fs::write(
            home.join("models_cache.json"),
            serde_json::json!({
                "models": [{
                    "slug": "gpt-5.6-terra",
                    "base_instructions": format!(
                        "You are Codex, an agent based on GPT-5.\n\n# Personality\n{}",
                        "c".repeat(4000)
                    )
                }]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            home.join("user-instructions.md"),
            "[MODE: UNRESTRICTED]\nfrom file",
        )
        .unwrap();
        fs::write(
            home.join("config.toml"),
            "model_instructions_file = \"./user-instructions.md\"\n",
        )
        .unwrap();

        let mut bi = String::new();
        // Codex home must not override multi-route catalog identity.
        let _ = home;
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn compact_file_override_mapped() {
        let home = temp_home("compact");
        fs::write(home.join("compact-override.md"), "CUSTOM COMPACT PROMPT").unwrap();
        fs::write(
            home.join("config.toml"),
            "experimental_compact_prompt_file = \"./compact-override.md\"\n",
        )
        .unwrap();
        let mapped = resolve_compact_prompt_from(&home);
        assert_eq!(
            mapped.source,
            CompactPromptSource::ExperimentalCompactPromptFile
        );
        assert_eq!(mapped.text.trim(), "CUSTOM COMPACT PROMPT");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn compact_fallback_matches_official_template() {
        let home = temp_home("compact-fb");
        let mapped = resolve_compact_prompt_from(&home);
        assert_eq!(mapped.source, CompactPromptSource::OfficialFallback);
        assert_eq!(mapped.text, OFFICIAL_COMPACT_PROMPT_FALLBACK);
        let _ = fs::remove_dir_all(&home);
    }
}
