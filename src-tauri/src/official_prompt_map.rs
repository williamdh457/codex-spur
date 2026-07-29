//! Map Codex official / user-overridden prompts into Spur — never invent copy.
//!
//! Source priority for `base_instructions`:
//! 1. `config.toml` → `model_instructions_file` (user override)
//! 2. `config.toml` → inline `base_instructions`
//! 3. `models_cache.json` Desktop agent row (`gpt-5.6-terra` …)
//!
//! Compact prompt priority:
//! 1. `experimental_compact_prompt_file`
//! 2. inline `compact_prompt`
//! 3. OSS-aligned fallback constant (byte-identical to codex-rs compact template)
//!
//! Plan / Goal templates stay inside Desktop; Spur only passthroughs request bodies.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use toml_edit::DocumentMut;

/// Historical Spur/CC Switch one-liner that replaced full official system prompts.
/// Fingerprint only — never use as default body.
pub const POLLUTED_BASE_INSTRUCTIONS_STUB: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

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

const PREFERRED_CACHE_SLUGS: &[&str] = &[
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.5",
    "gpt-5.4-mini",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseInstructionsSource {
    ModelInstructionsFile,
    InlineBaseInstructions,
    ModelsCache,
}

#[derive(Debug, Clone)]
pub struct MappedBaseInstructions {
    pub text: String,
    pub source: BaseInstructionsSource,
    /// Relative or absolute path / slug for logs (never the full prompt).
    pub source_label: String,
}

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

/// True when `text` is the old stub or another non-official short placeholder.
pub fn is_polluted_stub(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if t == POLLUTED_BASE_INSTRUCTIONS_STUB {
        return true;
    }
    if t == "You are Codex." {
        return true;
    }
    // Single-line short "You are Codex…" headers are not full Desktop prompts.
    if t.len() < 2_000 && !t.contains('\n') && t.starts_with("You are Codex") {
        return true;
    }
    false
}

pub fn resolve_base_instructions() -> Result<MappedBaseInstructions> {
    resolve_base_instructions_from(&crate::codex_config::prompt_map_codex_home())
}

pub fn resolve_base_instructions_from(codex_home: &Path) -> Result<MappedBaseInstructions> {
    let config_path = codex_home.join("config.toml");
    let config_text = fs::read_to_string(&config_path).ok();
    let doc = config_text
        .as_deref()
        .and_then(|text| text.parse::<DocumentMut>().ok());

    if let Some(doc) = doc.as_ref() {
        if let Some(rel) = top_level_string(doc, "model_instructions_file") {
            let path = resolve_codex_relative(codex_home, &rel);
            if let Some(text) = read_non_empty_file(&path)? {
                return Ok(MappedBaseInstructions {
                    text,
                    source: BaseInstructionsSource::ModelInstructionsFile,
                    source_label: path.display().to_string(),
                });
            }
        }
        if let Some(inline) = top_level_string(doc, "base_instructions") {
            let text = inline.trim().to_string();
            if !text.is_empty() {
                return Ok(MappedBaseInstructions {
                    text,
                    source: BaseInstructionsSource::InlineBaseInstructions,
                    source_label: "config.toml#base_instructions".into(),
                });
            }
        }
    }

    let cache_path = codex_home.join("models_cache.json");
    if let Some((slug, text)) = read_models_cache_base_instructions(&cache_path)? {
        return Ok(MappedBaseInstructions {
            text,
            source: BaseInstructionsSource::ModelsCache,
            source_label: format!("models_cache.json#{slug}"),
        });
    }

    bail!(
        "无法映射官方 base_instructions：请配置 model_instructions_file，或登录 Codex Desktop 生成 {}，禁止回填短 stub",
        cache_path.display()
    )
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

/// Apply mapped official/user base_instructions onto a catalog row (live Codex home).
///
/// Always overwrites when mapping succeeds (so edits to model_instructions_file
/// land on the next heal/apply). On mapping failure, clears polluted stubs so
/// they cannot be re-published.
pub fn apply_mapped_base_instructions(model_base_instructions: &mut String) {
    match resolve_base_instructions() {
        Ok(mapped) => write_mapped(model_base_instructions, mapped),
        Err(error) => clear_polluted_on_map_failure(model_base_instructions, &error),
    }
}

/// Test/helper entry: map from an explicit Codex home (temp fixtures).
#[cfg_attr(not(test), allow(dead_code))]
pub fn apply_mapped_base_instructions_from(model_base_instructions: &mut String, codex_home: &Path) {
    match resolve_base_instructions_from(codex_home) {
        Ok(mapped) => write_mapped(model_base_instructions, mapped),
        Err(error) => clear_polluted_on_map_failure(model_base_instructions, &error),
    }
}

fn write_mapped(model_base_instructions: &mut String, mapped: MappedBaseInstructions) {
    if model_base_instructions.as_str() != mapped.text.as_str() {
        tracing::info!(
            source = ?mapped.source,
            label = %mapped.source_label,
            chars = mapped.text.len(),
            "mapped official base_instructions into catalog row"
        );
    }
    *model_base_instructions = mapped.text;
}

fn clear_polluted_on_map_failure(model_base_instructions: &mut String, error: &anyhow::Error) {
    if is_polluted_stub(model_base_instructions) {
        tracing::error!(
            %error,
            "official base_instructions mapping failed; clearing polluted stub"
        );
        model_base_instructions.clear();
    } else {
        tracing::warn!(
            %error,
            chars = model_base_instructions.len(),
            "official base_instructions mapping failed; keeping non-stub existing text"
        );
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

fn read_models_cache_base_instructions(path: &Path) -> Result<Option<(String, String)>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取 models_cache 失败：{}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("解析 models_cache 失败：{}", path.display()))?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("models_cache.json 缺少 models 数组"))?;

    for slug in PREFERRED_CACHE_SLUGS {
        if let Some(text) = model_base_instructions(models, slug) {
            return Ok(Some(((*slug).to_string(), text)));
        }
    }

    let mut best: Option<(String, String)> = None;
    for model in models {
        let slug = model
            .get("slug")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if slug.eq_ignore_ascii_case("codex-auto-review") {
            continue;
        }
        let Some(text) = model
            .get("base_instructions")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if is_polluted_stub(text) {
            continue;
        }
        match &best {
            None => best = Some((slug.to_string(), text.to_string())),
            Some((_, prev)) if text.len() > prev.len() => {
                best = Some((slug.to_string(), text.to_string()));
            }
            _ => {}
        }
    }
    Ok(best)
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

fn model_base_instructions(models: &[Value], slug: &str) -> Option<String> {
    for model in models {
        let row_slug = model
            .get("slug")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)?;
        if row_slug != slug {
            continue;
        }
        let text = model
            .get("base_instructions")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        if is_polluted_stub(text) {
            return None;
        }
        return Some(text.to_string());
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
    fn polluted_stub_detected() {
        assert!(is_polluted_stub(POLLUTED_BASE_INSTRUCTIONS_STUB));
        assert!(is_polluted_stub("You are Codex."));
        assert!(is_polluted_stub(""));
        assert!(!is_polluted_stub(&format!(
            "You are Codex, an agent based on GPT-5.\n\n# Personality\n{}",
            "x".repeat(3000)
        )));
    }

    #[test]
    fn model_instructions_file_wins_over_models_cache() {
        let home = temp_home("file-wins");
        // Make cache text long enough to not look polluted if chosen wrongly.
        let long_cache = format!(
            "FROM_CACHE_PROMPT_SHOULD_NOT_WIN\n\n# Personality\n{}",
            "c".repeat(3000)
        );
        fs::write(
            home.join("models_cache.json"),
            serde_json::json!({
                "models": [{
                    "slug": "gpt-5.6-terra",
                    "base_instructions": long_cache
                }]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(home.join("user-instructions.md"), "USER_FILE_PROMPT_BODY\nline2").unwrap();
        fs::write(
            home.join("config.toml"),
            "model_instructions_file = \"./user-instructions.md\"\n",
        )
        .unwrap();

        let mapped = resolve_base_instructions_from(&home).expect("map");
        assert_eq!(mapped.source, BaseInstructionsSource::ModelInstructionsFile);
        assert!(mapped.text.contains("USER_FILE_PROMPT_BODY"));
        assert!(!mapped.text.contains("FROM_CACHE"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn models_cache_used_when_no_file() {
        let home = temp_home("cache");
        let body = format!(
            "You are Codex, an agent based on GPT-5.\n\n# Personality\n{}",
            "p".repeat(4000)
        );
        fs::write(
            home.join("models_cache.json"),
            serde_json::json!({
                "models": [
                    {"slug": "codex-auto-review", "base_instructions": "review only"},
                    {"slug": "gpt-5.6-terra", "base_instructions": body}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let mapped = resolve_base_instructions_from(&home).expect("map");
        assert_eq!(mapped.source, BaseInstructionsSource::ModelsCache);
        assert!(mapped.source_label.contains("gpt-5.6-terra"));
        assert!(mapped.text.starts_with("You are Codex, an agent"));
        assert!(mapped.text.len() > 2000);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_sources_error_without_stub() {
        let home = temp_home("empty");
        let err = resolve_base_instructions_from(&home).unwrap_err();
        assert!(err.to_string().contains("无法映射"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn apply_clears_stub_when_mapping_fails() {
        let home = temp_home("clear-stub");
        let mut bi = POLLUTED_BASE_INSTRUCTIONS_STUB.to_string();
        apply_mapped_base_instructions_from(&mut bi, &home);
        assert!(bi.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn apply_overwrites_stub_with_mapped() {
        let home = temp_home("overwrite");
        let body = format!("OFFICIAL_MAPPED\n\n# General\n{}", "z".repeat(3000));
        fs::write(
            home.join("models_cache.json"),
            serde_json::json!({
                "models": [{"slug": "gpt-5.6-terra", "base_instructions": body}]
            })
            .to_string(),
        )
        .unwrap();
        let mut bi = POLLUTED_BASE_INSTRUCTIONS_STUB.to_string();
        apply_mapped_base_instructions_from(&mut bi, &home);
        assert!(bi.starts_with("OFFICIAL_MAPPED"));
        assert!(!is_polluted_stub(&bi));
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
