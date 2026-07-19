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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    pub id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortPreset {
    pub effort: ReasoningEffort,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub slug: String,
    pub display_name: String,
    pub description: Option<String>,
    pub default_reasoning_level: Option<ReasoningEffort>,
    pub supported_reasoning_levels: Vec<ReasoningEffortPreset>,
    pub shell_type: String,
    pub visibility: String,
    pub supported_in_api: bool,
    pub priority: i32,
    pub additional_speed_tiers: Vec<String>,
    pub service_tiers: Vec<serde_json::Value>,
    pub default_service_tier: Option<String>,
    pub availability_nux: Option<serde_json::Value>,
    pub upgrade: Option<serde_json::Value>,
    pub base_instructions: String,
    pub model_messages: Option<serde_json::Value>,
    pub include_skills_usage_instructions: bool,
    pub supports_reasoning_summary_parameter: bool,
    pub default_reasoning_summary: serde_json::Value,
    pub support_verbosity: bool,
    pub default_verbosity: Option<serde_json::Value>,
    pub apply_patch_tool_type: Option<String>,
    pub web_search_tool_type: String,
    pub truncation_policy: TruncationPolicy,
    pub supports_parallel_tool_calls: bool,
    pub supports_image_detail_original: bool,
    pub context_window: Option<i64>,
    pub max_context_window: Option<i64>,
    pub auto_compact_token_limit: Option<i64>,
    pub comp_hash: Option<String>,
    pub effective_context_window_percent: i64,
    pub experimental_supported_tools: Vec<String>,
    pub input_modalities: Vec<String>,
    pub supports_search_tool: bool,
    pub use_responses_lite: bool,
    pub auto_review_model_override: Option<String>,
    pub tool_mode: Option<String>,
    pub multi_agent_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationPolicy {
    pub mode: String,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<CatalogModel>,
}
