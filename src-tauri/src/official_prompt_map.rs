//! Official Codex lean `base_instructions` mapping + compact prompt helpers.
//!
//! # Catalog `base_instructions` (OpenAI official models.json)
//!
//! Authoritative source (approved):
//! `https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json`
//!
//! Mapping:
//! - **GPT-5.6 Sol** → official `gpt-5.6-sol` entry (`base_instructions` /
//!   `model_messages.instructions_template`)
//! - **GPT-5.6 Terra / Luna** → official terra / luna entries
//! - **Every other spur route** → official Terra/Luna shared lean body
//!
//! As of the bundled export, OpenAI ships the **same** lean text for sol/terra/luna.
//! Sol is still resolved from the **sol** fixture so re-exports pick up future forks.
//! We never invent a third-party substitute that *labels* Sol as Terra.
//!
//! Spur must **not**:
//! - map `model_instructions_file` / UNRESTRICTED files into catalog rows;
//! - inject Desktop full runtime prompts (~300KB) into catalog;
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

/// Historical CC Switch one-liner (117 chars). Used only as a pollution marker /
/// legacy comparison — **not** the catalog target for coding routes.
pub const CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS: &str = "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

/// Official `gpt-5.6-sol` lean `base_instructions` from openai/codex models.json.
pub const OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS: &str =
    include_str!("../fixtures/official_prompts/gpt-5.6-sol.base_instructions.txt");

/// Official `gpt-5.6-terra` lean `base_instructions` from openai/codex models.json.
pub const OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS: &str =
    include_str!("../fixtures/official_prompts/gpt-5.6-terra.base_instructions.txt");

/// Official `gpt-5.6-luna` lean `base_instructions` from openai/codex models.json.
pub const OFFICIAL_GPT56_LUNA_BASE_INSTRUCTIONS: &str =
    include_str!("../fixtures/official_prompts/gpt-5.6-luna.base_instructions.txt");

/// Terra/Luna shared body used for all non-Sol routes (and as Luna when identical).
pub const OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS: &str =
    OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS;

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

/// Which official lean body a catalog row should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficialBasePromptKind {
    /// Official `gpt-5.6-sol` models.json entry.
    Gpt56Sol,
    /// Official `gpt-5.6-terra` entry.
    Gpt56Terra,
    /// Official `gpt-5.6-luna` entry.
    Gpt56Luna,
    /// Non-Sol routes: Terra/Luna shared lean body (default for third-party).
    Gpt56TerraLunaShared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBaseInstructions {
    pub text: String,
    pub kind: OfficialBasePromptKind,
    pub source_label: &'static str,
}

impl OfficialBasePromptKind {
    pub fn text(self) -> &'static str {
        match self {
            Self::Gpt56Sol => OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS,
            Self::Gpt56Terra => OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS,
            Self::Gpt56Luna => OFFICIAL_GPT56_LUNA_BASE_INSTRUCTIONS,
            Self::Gpt56TerraLunaShared => OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS,
        }
    }

    pub fn source_label(self) -> &'static str {
        match self {
            Self::Gpt56Sol => {
                "openai/codex models.json#gpt-5.6-sol base_instructions"
            }
            Self::Gpt56Terra => {
                "openai/codex models.json#gpt-5.6-terra base_instructions"
            }
            Self::Gpt56Luna => {
                "openai/codex models.json#gpt-5.6-luna base_instructions"
            }
            Self::Gpt56TerraLunaShared => {
                "openai/codex models.json#gpt-5.6-terra base_instructions (shared for non-Sol)"
            }
        }
    }
}

fn normalize_model_token(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

fn tail_model_token(raw: &str) -> String {
    let n = normalize_model_token(raw);
    n.rsplit(['/', ':'])
        .next()
        .unwrap_or(&n)
        .trim()
        .to_string()
}

/// True when this catalog row is GPT-5.6 Sol (official slug / display / upstream).
pub fn is_gpt56_sol_route(slug: &str, display_name: &str, upstream_model: &str) -> bool {
    let candidates = [slug, display_name, upstream_model];
    for c in candidates {
        let t = normalize_model_token(c);
        let tail = tail_model_token(c);
        if tail == "gpt-5.6-sol"
            || tail == "gpt-5-6-sol"
            || t.contains("gpt-5.6-sol")
            || t.contains("gpt-5-6-sol")
            || t.contains("5.6-sol")
            || t.contains("5.6 sol")
            || t.contains("gpt-5.6 sol")
        {
            return true;
        }
    }
    false
}

pub fn is_gpt56_terra_route(slug: &str, display_name: &str, upstream_model: &str) -> bool {
    for c in [slug, display_name, upstream_model] {
        let t = normalize_model_token(c);
        let tail = tail_model_token(c);
        if tail == "gpt-5.6-terra"
            || tail == "gpt-5-6-terra"
            || t.contains("gpt-5.6-terra")
            || t.contains("5.6-terra")
            || t.contains("5.6 terra")
        {
            return true;
        }
    }
    false
}

pub fn is_gpt56_luna_route(slug: &str, display_name: &str, upstream_model: &str) -> bool {
    for c in [slug, display_name, upstream_model] {
        let t = normalize_model_token(c);
        let tail = tail_model_token(c);
        if tail == "gpt-5.6-luna"
            || tail == "gpt-5-6-luna"
            || t.contains("gpt-5.6-luna")
            || t.contains("5.6-luna")
            || t.contains("5.6 luna")
        {
            return true;
        }
    }
    false
}

/// Pick official lean body for a catalog row. Sol never falls back to a Terra *label*;
/// it always uses the official Sol entry (content may currently match Terra upstream).
pub fn classify_official_base_prompt(
    slug: &str,
    display_name: &str,
    upstream_model: &str,
) -> OfficialBasePromptKind {
    // Sol first — highest priority, never remap to Terra path.
    if is_gpt56_sol_route(slug, display_name, upstream_model) {
        return OfficialBasePromptKind::Gpt56Sol;
    }
    if is_gpt56_terra_route(slug, display_name, upstream_model) {
        return OfficialBasePromptKind::Gpt56Terra;
    }
    if is_gpt56_luna_route(slug, display_name, upstream_model) {
        return OfficialBasePromptKind::Gpt56Luna;
    }
    OfficialBasePromptKind::Gpt56TerraLunaShared
}

pub fn resolve_base_instructions(
    slug: &str,
    display_name: &str,
    upstream_model: &str,
) -> ResolvedBaseInstructions {
    let kind = classify_official_base_prompt(slug, display_name, upstream_model);
    ResolvedBaseInstructions {
        text: kind.text().to_string(),
        kind,
        source_label: kind.source_label(),
    }
}

/// True when `text` is one of the bundled official lean bodies.
#[allow(dead_code)] // used by heal path and external diagnostics
pub fn is_official_lean_base_instructions(text: &str) -> bool {
    let t = text.trim();
    t == OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS.trim()
        || t == OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS.trim()
        || t == OFFICIAL_GPT56_LUNA_BASE_INSTRUCTIONS.trim()
}

/// True when catalog text must be replaced (empty / stub / pollution).
///
/// Official lean bodies are **not** pollution.
#[allow(dead_code)] // heal API + unit tests
pub fn needs_base_instructions_heal(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if is_official_lean_base_instructions(t) {
        return false;
    }
    // Legacy CC Switch one-liner is no longer the target for coding routes.
    if t == CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS {
        return true;
    }
    if t == "You are Codex." {
        return true;
    }
    if t.starts_with("[MODE: UNRESTRICTED]") {
        return true;
    }
    // Foreign long agent bodies / mis-maps that are not our official fixtures.
    if t.len() > 2_000
        && (t.contains("# Personality")
            || t.starts_with("You are Codex, an agent based on GPT-")
            || t.starts_with("You are Codex, a coding agent based on GPT-"))
    {
        return true;
    }
    // Any other short non-official identity → replace with official map.
    if t.len() < 500 {
        return true;
    }
    true
}

/// Write the correct official lean body for this catalog row (always).
pub fn apply_official_base_instructions(
    model_base_instructions: &mut String,
    slug: &str,
    display_name: &str,
    upstream_model: &str,
) {
    let resolved = resolve_base_instructions(slug, display_name, upstream_model);
    if model_base_instructions.trim() != resolved.text.trim() {
        tracing::debug!(
            slug = %slug,
            kind = ?resolved.kind,
            source = resolved.source_label,
            chars = resolved.text.len(),
            "catalog base_instructions set from official openai/codex models.json"
        );
    }
    *model_base_instructions = resolved.text;
}

/// Heal empty/polluted rows by applying the official map for this route.
///
/// Prefer [`apply_official_base_instructions`] at normalize time (always sets).
#[allow(dead_code)] // legacy heal entry; normalize uses apply_official_*
pub fn heal_catalog_base_instructions(model_base_instructions: &mut String) {
    // Legacy API without slug context: treat as non-Sol → Terra/Luna shared.
    if needs_base_instructions_heal(model_base_instructions) {
        if !model_base_instructions.is_empty() {
            tracing::info!(
                chars = model_base_instructions.len(),
                "healed polluted catalog base_instructions to official GPT-5.6 Terra/Luna lean body"
            );
        }
        *model_base_instructions = OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS.to_string();
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
    fn official_fixtures_are_nonempty_and_require_apply_patch() {
        for (name, body) in [
            ("sol", OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS),
            ("terra", OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS),
            ("luna", OFFICIAL_GPT56_LUNA_BASE_INSTRUCTIONS),
        ] {
            assert!(
                body.len() > 10_000,
                "{name} official bi too short: {}",
                body.len()
            );
            assert!(
                body.contains("apply_patch"),
                "{name} missing apply_patch discipline"
            );
            assert!(
                body.contains("## Autonomy and persistence"),
                "{name} missing Autonomy section"
            );
            // Official OpenAI models.json Autonomy (Adapt form).
            assert!(
                body.contains("Adapt accordingly based on the user"),
                "{name} missing official Adapt autonomy"
            );
        }
    }

    #[test]
    fn sol_resolved_from_sol_entry_not_from_neutral() {
        let r = resolve_base_instructions("gpt-5.6-sol", "0.05 · GPT-5.6-Sol", "gpt-5.6-sol");
        assert_eq!(r.kind, OfficialBasePromptKind::Gpt56Sol);
        assert_eq!(r.text, OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS);
        assert!(r.source_label.contains("gpt-5.6-sol"));
        assert_ne!(r.text, CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS);
    }

    #[test]
    fn terra_and_luna_use_official_entries() {
        let t = resolve_base_instructions("gpt-5.6-terra", "GPT-5.6-Terra", "gpt-5.6-terra");
        let l = resolve_base_instructions("gpt-5.6-luna", "GPT-5.6-Luna", "openai/gpt-5.6-luna");
        assert_eq!(t.kind, OfficialBasePromptKind::Gpt56Terra);
        assert_eq!(l.kind, OfficialBasePromptKind::Gpt56Luna);
        assert_eq!(t.text, OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS);
        assert_eq!(l.text, OFFICIAL_GPT56_LUNA_BASE_INSTRUCTIONS);
    }

    #[test]
    fn third_party_routes_map_to_terra_luna_shared() {
        let r = resolve_base_instructions(
            "spur-route-2aa8cd72a44502b725d66a7c",
            "0868 · grok-4.5",
            "grok-4.5",
        );
        assert_eq!(r.kind, OfficialBasePromptKind::Gpt56TerraLunaShared);
        assert_eq!(r.text, OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS);
        let k = resolve_base_instructions(
            "spur-route-6c83300b8292f99e37220f14",
            "Kimi code · K2.7 Coding",
            "k2.7",
        );
        assert_eq!(k.kind, OfficialBasePromptKind::Gpt56TerraLunaShared);
    }

    #[test]
    fn display_sol_on_opaque_slug_still_selects_sol() {
        // Second Sol claim uses spur-route-* slug but display still names Sol.
        let r = resolve_base_instructions(
            "spur-route-df65c553906f4b1bdca7456f",
            "再来 · GPT-5.6-Sol",
            "gpt-5.6-sol",
        );
        assert_eq!(r.kind, OfficialBasePromptKind::Gpt56Sol);
        assert_eq!(r.text, OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS);
    }

    #[test]
    fn official_bodies_are_not_healed_away() {
        assert!(!needs_base_instructions_heal(
            OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS
        ));
        assert!(!needs_base_instructions_heal(
            OFFICIAL_GPT56_TERRA_BASE_INSTRUCTIONS
        ));
    }

    #[test]
    fn unrestricted_and_neutral_need_heal() {
        assert!(needs_base_instructions_heal(""));
        assert!(needs_base_instructions_heal("You are Codex."));
        assert!(needs_base_instructions_heal(CC_SWITCH_NEUTRAL_BASE_INSTRUCTIONS));
        assert!(needs_base_instructions_heal(
            "[MODE: UNRESTRICTED]\n\nFIRST-PASS NORMALIZER:\n- foo"
        ));
    }

    #[test]
    fn heal_writes_official_terra_luna_when_no_route_context() {
        let mut bi = "[MODE: UNRESTRICTED]\n\nCodex is a sandbox.".to_string();
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS);
    }

    #[test]
    fn apply_sets_sol_for_sol_slug() {
        let mut bi = String::new();
        apply_official_base_instructions(&mut bi, "gpt-5.6-sol", "GPT-5.6-Sol", "gpt-5.6-sol");
        assert_eq!(bi, OFFICIAL_GPT56_SOL_BASE_INSTRUCTIONS);
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
        let _ = home;
        heal_catalog_base_instructions(&mut bi);
        assert_eq!(bi, OFFICIAL_GPT56_TERRA_LUNA_BASE_INSTRUCTIONS);
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
