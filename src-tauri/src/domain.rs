use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    pub running: bool,
    pub base_url: Option<String>,
    pub port: Option<u16>,
    pub catalog_revision: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBindingStatus {
    pub state: String,
    pub codex_home: String,
    pub provider_id: String,
    pub catalog_path: String,
}

/// One Desktop model-picker readiness check (Overview checklist).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopVisibilityCheck {
    pub id: String,
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

/// Whether ChatGPT Desktop can show custom catalog rows (Kimi/DeepSeek).
/// Distinct from Spur vault OAuth — identity lives in `~/.codex/auth.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopVisibility {
    /// True when auth + applied gate + catalog are healthy (list can show custom after cold start).
    pub ready: bool,
    /// Short metric label: 就绪 / 缺登录 / 待应用 / 异常
    pub status_label: String,
    pub codex_home: String,
    pub checks: Vec<DesktopVisibilityCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub region: String,
    pub protocol: String,
    pub configured: bool,
    pub selected_models: u32,
    pub discovered_models: u32,
    pub last_fetched_at: Option<String>,
    pub base_url: Option<String>,
    pub default_base_url: Option<String>,
    pub supports_official_account: bool,
    pub credential_count: u32,
    pub healthy_credential_count: u32,
    pub pool_count: u32,
    pub active_pool_id: Option<String>,
    /// `pool` or `fixed` — multi-account routing mode.
    pub routing_mode: String,
    pub fixed_credential_id: Option<String>,
    /// Primary entry channel for list badges: `official` | `json` | `api`.
    /// Legacy values `pool` / `config` are normalized to `json` when read.
    pub entry_category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningMapping {
    pub codex_effort: ReasoningEffort,
    pub upstream_effort: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningProfile {
    pub title: String,
    pub mappings: Vec<ReasoningMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRouteSummary {
    pub id: String,
    pub provider_id: String,
    /// User-facing provider instance name (`providers.name`).
    pub provider_name: String,
    pub upstream_model: String,
    pub display_name: String,
    pub enabled: bool,
    pub protocol: String,
    pub base_url: String,
    pub reasoning_profile: ReasoningProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSummary {
    pub id: String,
    pub provider_id: String,
    pub kind: String,
    pub state: String,
    pub label: Option<String>,
    pub masked_email: Option<String>,
    pub masked_account_id: Option<String>,
    pub expires_at: Option<i64>,
    pub fingerprint_prefix: String,
    pub refreshable: bool,
    pub healthy: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPoolSummary {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub strategy: String,
    pub sticky_ttl_secs: i64,
    pub enabled: bool,
    pub account_count: u32,
    pub healthy_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRouting {
    pub provider_id: String,
    pub routing_mode: String,
    pub fixed_credential_id: Option<String>,
    pub active_pool_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyRequestEvent {
    pub id: String,
    pub created_at: String,
    pub route_slug: Option<String>,
    pub display_name: Option<String>,
    pub provider_id: Option<String>,
    pub upstream_model: Option<String>,
    pub protocol: Option<String>,
    pub selection_layer: String,
    pub sticky_escaped: bool,
    pub account_fingerprint: Option<String>,
    pub schedule_state: Option<String>,
    pub result_category: String,
    pub failover_attempt: u32,
    pub latency_ms_total: Option<i64>,
    pub first_token_ms: Option<i64>,
    pub cooldown_applied: bool,
    pub error_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolMemberDetail {
    pub pool_id: String,
    pub credential_id: String,
    pub weight: i64,
    pub priority: i64,
    pub enabled: bool,
    pub concurrency_limit: i64,
    pub label: Option<String>,
    pub masked_email: Option<String>,
    pub healthy: bool,
    pub schedule_state: String,
    pub cooldown_until: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub reset_at: Option<i64>,
    pub window_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditsSummary {
    pub available_count: Option<i64>,
    pub credits: Vec<ResetCreditSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCreditSummary {
    pub granted_at: Option<i64>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiQuotaSnapshot {
    pub credential_id: String,
    pub plan_type: Option<String>,
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
    pub reset_credits: Option<ResetCreditsSummary>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub today_tokens: u64,
    pub seven_day_tokens: u64,
    pub cache_hit_rate: Option<f64>,
    pub failed_requests: u64,
    pub sampled_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub proxy: ProxyStatus,
    pub binding: CodexBindingStatus,
    pub providers: Vec<ProviderSummary>,
    pub published_models: u32,
    pub healthy_accounts: u32,
    pub attention_items: Vec<String>,
    pub desktop_visibility: DesktopVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexApplyOutcome {
    pub config_path: String,
    pub catalog_path: String,
    pub backup_path: Option<String>,
    pub before_hash: Option<String>,
    pub after_hash: String,
    pub restart_required: bool,
    pub model_count: u32,
    pub selected_model: Option<String>,
    /// Display names written into the catalog (for toast / diagnostics).
    #[serde(default)]
    pub model_labels: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyPreview {
    pub provider_id: String,
    pub base_url: String,
    pub catalog_path: String,
    pub selected_model: Option<String>,
    pub model_count: u32,
    pub toml_preview: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl ReasoningEffort {
    /// Full internal ladder used by proxy reasoning patches.
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
        Self::Ultra,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

/// Codex model-catalog entries use snake_case (see Nice/CC Switch catalogs).
/// Aliases accept legacy camelCase rows already stored in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ReasoningEffortPreset {
    pub effort: ReasoningEffort,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CatalogModel {
    pub slug: String,
    #[serde(alias = "displayName")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, alias = "defaultReasoningLevel", skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<ReasoningEffort>,
    #[serde(alias = "supportedReasoningLevels")]
    pub supported_reasoning_levels: Vec<ReasoningEffortPreset>,
    #[serde(alias = "shellType")]
    pub shell_type: String,
    pub visibility: String,
    #[serde(alias = "supportedInApi")]
    pub supported_in_api: bool,
    pub priority: i32,
    #[serde(default, alias = "additionalSpeedTiers")]
    pub additional_speed_tiers: Vec<String>,
    #[serde(default, alias = "serviceTiers")]
    pub service_tiers: Vec<serde_json::Value>,
    #[serde(default, alias = "defaultServiceTier", skip_serializing_if = "Option::is_none")]
    pub default_service_tier: Option<String>,
    #[serde(default, alias = "availabilityNux")]
    pub availability_nux: Option<serde_json::Value>,
    #[serde(default)]
    pub upgrade: Option<serde_json::Value>,
    #[serde(default, alias = "baseInstructions", skip_serializing_if = "String::is_empty")]
    pub base_instructions: String,
    #[serde(default, alias = "modelMessages", skip_serializing_if = "Option::is_none")]
    pub model_messages: Option<serde_json::Value>,
    // Bools must always serialize: Codex ModelInfo rejects missing required fields
    // (e.g. `support_verbosity`) even when the value is false.
    #[serde(default, alias = "includeSkillsUsageInstructions")]
    pub include_skills_usage_instructions: bool,
    /// Working third-party catalogs expose this name.
    #[serde(default, alias = "supportsReasoningSummaries")]
    pub supports_reasoning_summaries: bool,
    #[serde(default, alias = "supportsReasoningSummaryParameter")]
    pub supports_reasoning_summary_parameter: bool,
    #[serde(default = "default_reasoning_summary_value", alias = "defaultReasoningSummary")]
    pub default_reasoning_summary: serde_json::Value,
    #[serde(default, alias = "supportVerbosity")]
    pub support_verbosity: bool,
    #[serde(default, alias = "defaultVerbosity", skip_serializing_if = "Option::is_none")]
    pub default_verbosity: Option<serde_json::Value>,
    #[serde(default, alias = "applyPatchToolType", skip_serializing_if = "Option::is_none")]
    pub apply_patch_tool_type: Option<String>,
    #[serde(
        default,
        alias = "webSearchToolType",
        skip_serializing_if = "Option::is_none"
    )]
    pub web_search_tool_type: Option<String>,
    #[serde(default, alias = "truncationPolicy")]
    pub truncation_policy: TruncationPolicy,
    #[serde(default, alias = "supportsParallelToolCalls")]
    pub supports_parallel_tool_calls: bool,
    #[serde(default, alias = "supportsImageDetailOriginal")]
    pub supports_image_detail_original: bool,
    #[serde(default, alias = "contextWindow", skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(default, alias = "maxContextWindow", skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<i64>,
    #[serde(default, alias = "autoCompactTokenLimit", skip_serializing_if = "Option::is_none")]
    pub auto_compact_token_limit: Option<i64>,
    #[serde(default, alias = "compHash", skip_serializing_if = "Option::is_none")]
    pub comp_hash: Option<String>,
    #[serde(default = "default_effective_context_window_percent", alias = "effectiveContextWindowPercent")]
    pub effective_context_window_percent: i64,
    // Codex requires this field present (even as []) — do not skip when empty.
    #[serde(default, alias = "experimentalSupportedTools")]
    pub experimental_supported_tools: Vec<String>,
    #[serde(default = "default_input_modalities", alias = "inputModalities")]
    pub input_modalities: Vec<String>,
    #[serde(default, alias = "supportsSearchTool")]
    pub supports_search_tool: bool,
    #[serde(default, alias = "useResponsesLite")]
    pub use_responses_lite: bool,
    #[serde(default, alias = "autoReviewModelOverride", skip_serializing_if = "Option::is_none")]
    pub auto_review_model_override: Option<String>,
    #[serde(default, alias = "toolMode", skip_serializing_if = "Option::is_none")]
    pub tool_mode: Option<String>,
    #[serde(default, alias = "multiAgentVersion", skip_serializing_if = "Option::is_none")]
    pub multi_agent_version: Option<String>,
}

fn default_reasoning_summary_value() -> serde_json::Value {
    serde_json::Value::String("auto".into())
}

fn default_effective_context_window_percent() -> i64 {
    95
}

fn default_input_modalities() -> Vec<String> {
    vec!["text".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct TruncationPolicy {
    pub mode: String,
    pub limit: i64,
}

impl Default for TruncationPolicy {
    fn default() -> Self {
        Self {
            mode: "tokens".into(),
            limit: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<CatalogModel>,
}
