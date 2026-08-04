//! User-selected reasoning mapping templates (provider-level).
//!
//! Codex Desktop effort ladder is product-facing. Each template maps the eight
//! Codex rungs onto real upstream tokens and decides which subset the catalog
//! should advertise. Templates are **chosen by the user** on the provider
//! instance — model-name heuristics must never override that choice.

use serde::{Deserialize, Serialize};

use crate::domain::{ReasoningEffort, ReasoningEffortPreset, ReasoningMapping, ReasoningProfile};

/// Stable id stored on `providers.reasoning_profile_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningProfileId {
    OpenaiNative,
    OpenaiCompat,
    Deepseek,
    Kimi,
    Xai,
    Minimax,
    Glm,
    Qwen,
    Passthrough,
}

/// The concrete upstream mechanism for a specific route.  Provider templates are
/// still useful fallbacks, but K3/K2.7/Grok have materially different contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningUpstreamMode {
    ResponsesReasoningEffort,
    ChatReasoningEffort,
    ThinkingToggle,
    AlwaysOn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReasoningCapability {
    pub selectable: bool,
    pub levels: Vec<ReasoningEffort>,
    pub default_level: Option<ReasoningEffort>,
    pub upstream_mode: ReasoningUpstreamMode,
}

fn model_tail(model_id: &str) -> String {
    model_id
        .trim()
        .to_ascii_lowercase()
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// Resolve documented model-specific behavior before falling back to a provider
/// template. Do not infer this from a display name: only stable upstream ids.
pub fn model_reasoning_capability(
    kind: &str,
    upstream_model: &str,
    profile: ReasoningProfileId,
) -> ModelReasoningCapability {
    let model = model_tail(upstream_model);
    if kind.eq_ignore_ascii_case("kimi") {
        if model == "k3" || model == "kimi-k3" {
            return ModelReasoningCapability {
                selectable: true,
                levels: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::High,
                    ReasoningEffort::Max,
                ],
                default_level: Some(ReasoningEffort::Max),
                upstream_mode: ReasoningUpstreamMode::ChatReasoningEffort,
            };
        }
        if matches!(
            model.as_str(),
            "kimi-for-coding" | "kimi-k2.7-code" | "kimi-k2.7-code-highspeed"
        ) {
            return ModelReasoningCapability {
                selectable: false,
                levels: vec![ReasoningEffort::Medium],
                default_level: Some(ReasoningEffort::Medium),
                upstream_mode: ReasoningUpstreamMode::AlwaysOn,
            };
        }
        if matches!(model.as_str(), "kimi-k2.6" | "kimi-k2.5") {
            return ModelReasoningCapability {
                selectable: true,
                levels: vec![ReasoningEffort::None, ReasoningEffort::Medium],
                default_level: Some(ReasoningEffort::Medium),
                upstream_mode: ReasoningUpstreamMode::ThinkingToggle,
            };
        }
    }
    if kind.eq_ignore_ascii_case("xai") && model.starts_with("grok-4.5") {
        return ModelReasoningCapability {
            selectable: true,
            levels: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            default_level: Some(ReasoningEffort::High),
            upstream_mode: ReasoningUpstreamMode::ResponsesReasoningEffort,
        };
    }
    ModelReasoningCapability {
        selectable: true,
        levels: catalog_reasoning_levels(profile)
            .into_iter()
            .map(|level| level.effort)
            .collect(),
        default_level: Some(default_reasoning_level(profile)),
        upstream_mode: match profile {
            ReasoningProfileId::Deepseek
            | ReasoningProfileId::Minimax
            | ReasoningProfileId::Glm
            | ReasoningProfileId::Qwen => ReasoningUpstreamMode::ThinkingToggle,
            _ => ReasoningUpstreamMode::ResponsesReasoningEffort,
        },
    }
}

impl ReasoningProfileId {
    pub const ALL: [Self; 9] = [
        Self::OpenaiNative,
        Self::OpenaiCompat,
        Self::Deepseek,
        Self::Kimi,
        Self::Xai,
        Self::Minimax,
        Self::Glm,
        Self::Qwen,
        Self::Passthrough,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiNative => "openai_native",
            Self::OpenaiCompat => "openai_compat",
            Self::Deepseek => "deepseek",
            Self::Kimi => "kimi",
            Self::Xai => "xai",
            Self::Minimax => "minimax",
            Self::Glm => "glm",
            Self::Qwen => "qwen",
            Self::Passthrough => "passthrough",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "openai_native" | "openai" => Some(Self::OpenaiNative),
            "openai_compat" | "compat" => Some(Self::OpenaiCompat),
            "deepseek" => Some(Self::Deepseek),
            "kimi" => Some(Self::Kimi),
            "xai" | "grok" => Some(Self::Xai),
            "minimax" => Some(Self::Minimax),
            "glm" | "zhipu" => Some(Self::Glm),
            "qwen" => Some(Self::Qwen),
            "passthrough" | "identity" => Some(Self::Passthrough),
            _ => None,
        }
    }

    /// Default template when a dedicated provider kind is created.
    pub fn default_for_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "openai" => Self::OpenaiNative,
            "deepseek" => Self::Deepseek,
            "kimi" => Self::Kimi,
            "xai" => Self::Xai,
            "minimax" => Self::Minimax,
            // custom / opencode-go / unknown: user should pick; default GPT-oriented.
            "custom" | "opencode-go" => Self::OpenaiNative,
            _ => Self::OpenaiNative,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::OpenaiNative => "OpenAI / Codex 原生",
            Self::OpenaiCompat => "OpenAI 兼容（保守）",
            Self::Deepseek => "DeepSeek",
            Self::Kimi => "Kimi",
            Self::Xai => "xAI Grok",
            Self::Minimax => "MiniMax",
            Self::Glm => "智谱 GLM",
            Self::Qwen => "通义 Qwen",
            Self::Passthrough => "透传（不改写）",
        }
    }

    pub fn short_help(self) -> &'static str {
        match self {
            Self::OpenaiNative => "官方/真 GPT：none…ultra 恒等；Catalog 跟 models_cache",
            Self::OpenaiCompat => "杂牌中转：只稳 low/medium/high；超高夹到 high",
            Self::Deepseek => "V4：关思考 / high / max",
            Self::Kimi => "off / low / medium / high",
            Self::Xai => "Grok 4.5：low / medium / high（无法关闭）",
            Self::Minimax => "thinking：disabled / adaptive / enabled",
            Self::Glm => "关思考 / high / max",
            Self::Qwen => "enable_thinking 开/关",
            Self::Passthrough => "原样转发 effort（风险自负）",
        }
    }
}

/// One upstream patch applied to a request body for a single Codex effort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamReasoningPatch {
    /// Value written to `reasoning.effort` (Responses) when Some.
    pub responses_effort: Option<&'static str>,
    /// Value written to Chat Completions `reasoning_effort` when Some.
    pub chat_reasoning_effort: Option<&'static str>,
    /// Optional `thinking: { "type": ... }` (DeepSeek / GLM / MiniMax).
    pub thinking_type: Option<&'static str>,
    /// Optional top-level `enable_thinking` (Qwen).
    pub enable_thinking: Option<bool>,
    /// Drop reasoning knobs entirely (none/minimal omit paths).
    pub omit_reasoning: bool,
}

impl UpstreamReasoningPatch {
    fn effort_only(effort: &'static str) -> Self {
        Self {
            responses_effort: Some(effort),
            chat_reasoning_effort: Some(effort),
            thinking_type: None,
            enable_thinking: None,
            omit_reasoning: false,
        }
    }

    fn omit() -> Self {
        Self {
            responses_effort: None,
            chat_reasoning_effort: None,
            thinking_type: None,
            enable_thinking: None,
            omit_reasoning: true,
        }
    }
}

/// Resolve the patch for one Codex effort under a user-selected template.
pub fn patch_for(profile: ReasoningProfileId, codex_effort: &str) -> UpstreamReasoningPatch {
    let effort = normalize_codex_effort(codex_effort);
    match profile {
        ReasoningProfileId::OpenaiNative | ReasoningProfileId::Passthrough => {
            if let Some(e) = effort {
                UpstreamReasoningPatch::effort_only(e)
            } else {
                UpstreamReasoningPatch::omit()
            }
        }
        ReasoningProfileId::OpenaiCompat => match effort {
            Some("none") | Some("minimal") => UpstreamReasoningPatch::omit(),
            Some("low") => UpstreamReasoningPatch::effort_only("low"),
            Some("medium") => UpstreamReasoningPatch::effort_only("medium"),
            Some("high") | Some("xhigh") | Some("max") | Some("ultra") => {
                UpstreamReasoningPatch::effort_only("high")
            }
            _ => UpstreamReasoningPatch::omit(),
        },
        ReasoningProfileId::Deepseek => match effort {
            Some("none") => UpstreamReasoningPatch {
                responses_effort: None,
                chat_reasoning_effort: None,
                thinking_type: Some("disabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("minimal") | Some("low") | Some("medium") | Some("high") => {
                UpstreamReasoningPatch {
                    responses_effort: Some("high"),
                    chat_reasoning_effort: Some("high"),
                    thinking_type: Some("enabled"),
                    enable_thinking: None,
                    omit_reasoning: false,
                }
            }
            Some("xhigh") | Some("max") | Some("ultra") => UpstreamReasoningPatch {
                responses_effort: Some("max"),
                chat_reasoning_effort: Some("max"),
                thinking_type: Some("enabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            _ => UpstreamReasoningPatch::omit(),
        },
        ReasoningProfileId::Kimi => match effort {
            Some("none") => UpstreamReasoningPatch {
                responses_effort: Some("off"),
                chat_reasoning_effort: None,
                thinking_type: None,
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("minimal") | Some("low") => UpstreamReasoningPatch::effort_only("low"),
            Some("medium") => UpstreamReasoningPatch::effort_only("medium"),
            Some("high") | Some("xhigh") | Some("max") | Some("ultra") => {
                UpstreamReasoningPatch::effort_only("high")
            }
            _ => UpstreamReasoningPatch::omit(),
        },
        ReasoningProfileId::Xai => match effort {
            // Grok 4.5 cannot disable reasoning — clamp none/minimal to low.
            Some("none") | Some("minimal") | Some("low") => {
                UpstreamReasoningPatch::effort_only("low")
            }
            Some("medium") => UpstreamReasoningPatch::effort_only("medium"),
            Some("high") | Some("xhigh") | Some("max") | Some("ultra") => {
                UpstreamReasoningPatch::effort_only("high")
            }
            _ => UpstreamReasoningPatch::effort_only("high"),
        },
        ReasoningProfileId::Minimax => match effort {
            Some("none") | Some("minimal") => UpstreamReasoningPatch {
                responses_effort: Some("disabled"),
                chat_reasoning_effort: None,
                thinking_type: Some("disabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("low") | Some("medium") => UpstreamReasoningPatch {
                responses_effort: Some("adaptive"),
                chat_reasoning_effort: None,
                thinking_type: Some("adaptive"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("high") | Some("xhigh") | Some("max") | Some("ultra") => UpstreamReasoningPatch {
                responses_effort: Some("enabled"),
                chat_reasoning_effort: None,
                thinking_type: Some("enabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            _ => UpstreamReasoningPatch {
                responses_effort: Some("adaptive"),
                chat_reasoning_effort: None,
                thinking_type: Some("adaptive"),
                enable_thinking: None,
                omit_reasoning: false,
            },
        },
        ReasoningProfileId::Glm => match effort {
            Some("none") | Some("minimal") => UpstreamReasoningPatch {
                responses_effort: Some("none"),
                chat_reasoning_effort: None,
                thinking_type: Some("disabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("low") | Some("medium") | Some("high") => UpstreamReasoningPatch {
                responses_effort: Some("high"),
                chat_reasoning_effort: Some("high"),
                thinking_type: Some("enabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            Some("xhigh") | Some("max") | Some("ultra") => UpstreamReasoningPatch {
                responses_effort: Some("max"),
                chat_reasoning_effort: Some("max"),
                thinking_type: Some("enabled"),
                enable_thinking: None,
                omit_reasoning: false,
            },
            _ => UpstreamReasoningPatch::omit(),
        },
        ReasoningProfileId::Qwen => match effort {
            Some("none") | Some("minimal") => UpstreamReasoningPatch {
                responses_effort: None,
                chat_reasoning_effort: None,
                thinking_type: None,
                enable_thinking: Some(false),
                omit_reasoning: true,
            },
            Some(_) => UpstreamReasoningPatch {
                responses_effort: Some("medium"),
                chat_reasoning_effort: Some("medium"),
                thinking_type: None,
                enable_thinking: Some(true),
                omit_reasoning: false,
            },
            None => UpstreamReasoningPatch::omit(),
        },
    }
}

/// Resolve an upstream patch for a route. This is deliberately separate from
/// `patch_for`: a provider profile cannot accurately describe K3 and K2.7.
pub fn patch_for_model(
    kind: &str,
    upstream_model: &str,
    profile: ReasoningProfileId,
    codex_effort: &str,
) -> UpstreamReasoningPatch {
    let effort = normalize_codex_effort(codex_effort);
    let capability = model_reasoning_capability(kind, upstream_model, profile);
    match capability.upstream_mode {
        ReasoningUpstreamMode::AlwaysOn => UpstreamReasoningPatch::omit(),
        ReasoningUpstreamMode::ChatReasoningEffort
            if kind.eq_ignore_ascii_case("kimi")
                && matches!(model_tail(upstream_model).as_str(), "k3" | "kimi-k3") =>
        {
            let upstream = match effort {
                Some("low") => "low",
                Some("max") | Some("xhigh") | Some("ultra") => "max",
                // K3 cannot disable thinking and has no medium level.
                Some("none") | Some("minimal") | Some("medium") | Some("high") | None => "high",
                _ => "high",
            };
            UpstreamReasoningPatch {
                responses_effort: None,
                chat_reasoning_effort: Some(upstream),
                thinking_type: None,
                enable_thinking: None,
                omit_reasoning: false,
            }
        }
        ReasoningUpstreamMode::ThinkingToggle
            if kind.eq_ignore_ascii_case("kimi")
                && matches!(
                    model_tail(upstream_model).as_str(),
                    "kimi-k2.6" | "kimi-k2.5"
                ) =>
        {
            let disabled = matches!(effort, Some("none") | Some("minimal"));
            UpstreamReasoningPatch {
                responses_effort: None,
                chat_reasoning_effort: None,
                thinking_type: Some(if disabled { "disabled" } else { "enabled" }),
                enable_thinking: None,
                omit_reasoning: true,
            }
        }
        ReasoningUpstreamMode::ResponsesReasoningEffort
            if kind.eq_ignore_ascii_case("xai")
                && model_tail(upstream_model).starts_with("grok-4.5") =>
        {
            let upstream = match effort {
                Some("none") | Some("minimal") | Some("low") => "low",
                Some("medium") => "medium",
                _ => "high",
            };
            UpstreamReasoningPatch {
                responses_effort: Some(upstream),
                chat_reasoning_effort: None,
                thinking_type: None,
                enable_thinking: None,
                omit_reasoning: false,
            }
        }
        _ => patch_for(profile, codex_effort),
    }
}

fn normalize_codex_effort(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        "ultra" => Some("ultra"),
        // Legacy internal tokens still appear in stored profiles / tests.
        "disabled" | "off" => Some("none"),
        "enabled" | "default" => Some("medium"),
        _ => None,
    }
}

/// Display label written into `reasoning_profile.mappings[].upstream_effort`.
fn display_upstream(profile: ReasoningProfileId, effort: ReasoningEffort) -> String {
    let patch = patch_for(profile, effort.as_str());
    if patch.omit_reasoning && patch.thinking_type.is_none() && patch.enable_thinking.is_none() {
        return "omit".into();
    }
    if let Some(t) = patch.thinking_type {
        if let Some(e) = patch.chat_reasoning_effort.or(patch.responses_effort) {
            return format!("thinking:{t}+effort:{e}");
        }
        return format!("thinking:{t}");
    }
    if let Some(en) = patch.enable_thinking {
        return if en {
            "enable_thinking:true".into()
        } else {
            "enable_thinking:false".into()
        };
    }
    patch
        .responses_effort
        .or(patch.chat_reasoning_effort)
        .unwrap_or("omit")
        .to_string()
}

/// Full eight-row mapping card for Spur UI / stored catalog_json.
pub fn reasoning_profile(profile: ReasoningProfileId, model_id: &str) -> ReasoningProfile {
    let mappings = ReasoningEffort::ALL
        .into_iter()
        .map(|codex_effort| {
            let upstream = display_upstream(profile, codex_effort);
            ReasoningMapping {
                codex_effort,
                upstream_effort: upstream.clone(),
                explanation: format!(
                    "Codex {} → {} ({})",
                    codex_effort.as_str(),
                    profile.title(),
                    upstream
                ),
            }
        })
        .collect();
    ReasoningProfile {
        title: format!("{} · {model_id}", profile.title()),
        mappings,
    }
}

/// Codex-facing catalog levels that remain distinct after mapping.
pub fn catalog_reasoning_levels(profile: ReasoningProfileId) -> Vec<ReasoningEffortPreset> {
    let efforts: &[ReasoningEffort] = match profile {
        ReasoningProfileId::OpenaiNative | ReasoningProfileId::Passthrough => {
            // Prefer models_cache when available; fallback full product ladder without inventing
            // none/minimal for agent models is applied by caller. Here default to terra-like 6.
            &[
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
                ReasoningEffort::Max,
                ReasoningEffort::Ultra,
            ]
        }
        ReasoningProfileId::OpenaiCompat => &[
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        ReasoningProfileId::Deepseek => &[
            ReasoningEffort::None,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ],
        ReasoningProfileId::Kimi => &[
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        ReasoningProfileId::Xai => &[
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        ReasoningProfileId::Minimax => &[
            ReasoningEffort::None,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ],
        ReasoningProfileId::Glm => &[
            ReasoningEffort::None,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ],
        ReasoningProfileId::Qwen => &[ReasoningEffort::None, ReasoningEffort::Medium],
    };

    efforts
        .iter()
        .copied()
        .map(|effort| ReasoningEffortPreset {
            effort,
            description: catalog_level_description(profile, effort).into(),
        })
        .collect()
}

pub fn catalog_level_description_for_model(
    kind: &str,
    upstream_model: &str,
    profile: ReasoningProfileId,
    effort: ReasoningEffort,
) -> &'static str {
    let capability = model_reasoning_capability(kind, upstream_model, profile);
    match (capability.upstream_mode, effort) {
        (ReasoningUpstreamMode::ChatReasoningEffort, ReasoningEffort::Low)
            if kind.eq_ignore_ascii_case("kimi") =>
        {
            "Kimi K3 low reasoning effort"
        }
        (ReasoningUpstreamMode::ChatReasoningEffort, ReasoningEffort::High)
            if kind.eq_ignore_ascii_case("kimi") =>
        {
            "Kimi K3 high reasoning effort"
        }
        (ReasoningUpstreamMode::ChatReasoningEffort, ReasoningEffort::Max)
            if kind.eq_ignore_ascii_case("kimi") =>
        {
            "Kimi K3 maximum reasoning effort"
        }
        (ReasoningUpstreamMode::AlwaysOn, _) if kind.eq_ignore_ascii_case("kimi") => {
            "Thinking always enabled by this Kimi model"
        }
        (ReasoningUpstreamMode::ResponsesReasoningEffort, ReasoningEffort::Low)
            if kind.eq_ignore_ascii_case("xai") =>
        {
            "Grok low reasoning (cannot fully disable)"
        }
        (ReasoningUpstreamMode::ResponsesReasoningEffort, ReasoningEffort::Medium)
            if kind.eq_ignore_ascii_case("xai") =>
        {
            "Grok medium reasoning"
        }
        (ReasoningUpstreamMode::ResponsesReasoningEffort, ReasoningEffort::High)
            if kind.eq_ignore_ascii_case("xai") =>
        {
            "Grok high reasoning"
        }
        _ => catalog_level_description(profile, effort),
    }
}

fn catalog_level_description(profile: ReasoningProfileId, effort: ReasoningEffort) -> &'static str {
    match (profile, effort) {
        (ReasoningProfileId::Deepseek, ReasoningEffort::None) => "Disable thinking",
        (ReasoningProfileId::Deepseek, ReasoningEffort::High) => {
            "Thinking on · effort high (DeepSeek default)"
        }
        (ReasoningProfileId::Deepseek, ReasoningEffort::Max) => {
            "Thinking on · effort max (DeepSeek deepest)"
        }
        (ReasoningProfileId::Xai, ReasoningEffort::Low) => {
            "Grok low reasoning (cannot fully disable)"
        }
        (ReasoningProfileId::Minimax, ReasoningEffort::None) => "thinking disabled",
        (ReasoningProfileId::Minimax, ReasoningEffort::Medium) => "thinking adaptive",
        (ReasoningProfileId::Minimax, ReasoningEffort::High) => "thinking enabled",
        (ReasoningProfileId::Glm, ReasoningEffort::None) => "Disable deep thinking",
        (ReasoningProfileId::Glm, ReasoningEffort::High) => "Thinking on · effort high",
        (ReasoningProfileId::Glm, ReasoningEffort::Max) => "Thinking on · effort max",
        (ReasoningProfileId::Qwen, ReasoningEffort::None) => "enable_thinking off",
        (ReasoningProfileId::Qwen, ReasoningEffort::Medium) => "enable_thinking on",
        (ReasoningProfileId::OpenaiCompat, ReasoningEffort::High) => {
            "Greater reasoning depth (also used for xhigh/max/ultra on this gateway)"
        }
        _ => crate::providers::codex_reasoning_level_description(effort),
    }
}

pub fn default_reasoning_level(profile: ReasoningProfileId) -> ReasoningEffort {
    let levels = catalog_reasoning_levels(profile);
    if levels.iter().any(|l| l.effort == ReasoningEffort::Medium) {
        return ReasoningEffort::Medium;
    }
    if levels.iter().any(|l| l.effort == ReasoningEffort::High) {
        return ReasoningEffort::High;
    }
    levels
        .first()
        .map(|l| l.effort)
        .unwrap_or(ReasoningEffort::Medium)
}

/// Apply a patch to a Responses-style request (`reasoning.effort` + optional thinking fields).
pub fn apply_patch_to_responses_request(
    request: &mut serde_json::Value,
    patch: &UpstreamReasoningPatch,
) {
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if patch.omit_reasoning {
        if let Some(reasoning) = object.get_mut("reasoning").and_then(|v| v.as_object_mut()) {
            reasoning.remove("effort");
            if reasoning.is_empty() {
                object.remove("reasoning");
            }
        }
    } else if let Some(effort) = patch.responses_effort {
        let reasoning = object
            .entry("reasoning")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(map) = reasoning.as_object_mut() {
            map.insert("effort".into(), serde_json::Value::String(effort.into()));
        }
    }

    if let Some(thinking_type) = patch.thinking_type {
        object.insert(
            "thinking".into(),
            serde_json::json!({ "type": thinking_type }),
        );
    }
    if let Some(enable) = patch.enable_thinking {
        object.insert("enable_thinking".into(), serde_json::Value::Bool(enable));
    }
}

/// Apply a patch to a Chat Completions request body.
pub fn apply_patch_to_chat_request(chat: &mut serde_json::Value, patch: &UpstreamReasoningPatch) {
    let Some(object) = chat.as_object_mut() else {
        return;
    };
    if patch.omit_reasoning {
        object.remove("reasoning_effort");
    } else if let Some(effort) = patch.chat_reasoning_effort {
        object.insert(
            "reasoning_effort".into(),
            serde_json::Value::String(effort.into()),
        );
    } else {
        object.remove("reasoning_effort");
    }
    if let Some(thinking_type) = patch.thinking_type {
        object.insert(
            "thinking".into(),
            serde_json::json!({ "type": thinking_type }),
        );
    }
    if let Some(enable) = patch.enable_thinking {
        object.insert("enable_thinking".into(), serde_json::Value::Bool(enable));
    }
}

/// UI / IPC catalog of selectable templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningProfileOption {
    pub id: String,
    pub title: String,
    pub help: String,
}

pub fn list_reasoning_profile_options() -> Vec<ReasoningProfileOption> {
    ReasoningProfileId::ALL
        .into_iter()
        .map(|id| ReasoningProfileOption {
            id: id.as_str().into(),
            title: id.title().into(),
            help: id.short_help().into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_maps_xhigh_to_max_not_medium() {
        let patch = patch_for(ReasoningProfileId::Deepseek, "xhigh");
        assert_eq!(patch.chat_reasoning_effort, Some("max"));
        assert_eq!(patch.thinking_type, Some("enabled"));
        assert_ne!(patch.chat_reasoning_effort, Some("medium"));
    }

    #[test]
    fn openai_compat_clamps_xhigh_to_high() {
        let patch = patch_for(ReasoningProfileId::OpenaiCompat, "xhigh");
        assert_eq!(patch.responses_effort, Some("high"));
        assert_eq!(patch.chat_reasoning_effort, Some("high"));
    }

    #[test]
    fn openai_native_preserves_xhigh() {
        let patch = patch_for(ReasoningProfileId::OpenaiNative, "xhigh");
        assert_eq!(patch.responses_effort, Some("xhigh"));
    }

    #[test]
    fn every_profile_has_eight_mappings() {
        for id in ReasoningProfileId::ALL {
            assert_eq!(reasoning_profile(id, "m").mappings.len(), 8);
            assert!(!catalog_reasoning_levels(id).is_empty());
        }
    }

    #[test]
    fn default_for_kind_table() {
        assert_eq!(
            ReasoningProfileId::default_for_kind("deepseek"),
            ReasoningProfileId::Deepseek
        );
        assert_eq!(
            ReasoningProfileId::default_for_kind("custom"),
            ReasoningProfileId::OpenaiNative
        );
    }

    #[test]
    fn kimi_none_does_not_emit_chat_effort() {
        let patch = patch_for(ReasoningProfileId::Kimi, "none");
        assert!(patch.chat_reasoning_effort.is_none());
    }

    #[test]
    fn kimi_k3_uses_its_documented_three_levels_and_clamps_legacy_values() {
        let capability = model_reasoning_capability("kimi", "k3", ReasoningProfileId::Kimi);
        assert_eq!(
            capability.levels,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::High,
                ReasoningEffort::Max
            ]
        );
        assert_eq!(capability.default_level, Some(ReasoningEffort::Max));
        assert_eq!(
            patch_for_model("kimi", "k3", ReasoningProfileId::Kimi, "none").chat_reasoning_effort,
            Some("high")
        );
        assert_eq!(
            patch_for_model("kimi", "k3", ReasoningProfileId::Kimi, "xhigh").chat_reasoning_effort,
            Some("max")
        );
    }

    #[test]
    fn kimi_k27_drops_effort_and_grok_45_clamps_without_disabling() {
        let fixed = patch_for_model("kimi", "kimi-for-coding", ReasoningProfileId::Kimi, "max");
        assert!(fixed.omit_reasoning);

        let low = patch_for_model("xai", "grok-4.5", ReasoningProfileId::Xai, "none");
        assert_eq!(low.responses_effort, Some("low"));
        let high = patch_for_model("xai", "grok-4.5", ReasoningProfileId::Xai, "ultra");
        assert_eq!(high.responses_effort, Some("high"));
        assert!(high.chat_reasoning_effort.is_none());
    }
}
