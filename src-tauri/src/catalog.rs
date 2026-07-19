use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use crate::{
    domain::{
        CatalogModel, ModelsResponse, ReasoningEffort, ReasoningEffortPreset, TruncationPolicy,
    },
    providers::RouteCatalogPayload,
    storage::StoredRoute,
};

#[derive(Debug, Clone)]
pub struct RouteTarget {
    pub provider_id: String,
    pub upstream_model: String,
    pub base_url: String,
    pub protocol: String,
}

pub type SharedCatalog = Arc<RwLock<ModelsResponse>>;
pub type SharedRoutes = Arc<RwLock<HashMap<String, RouteTarget>>>;

pub fn build_from_routes(routes: &[StoredRoute]) -> (ModelsResponse, HashMap<String, RouteTarget>) {
    let mut models = Vec::new();
    let mut targets = HashMap::new();
    for route in routes.iter().filter(|route| route.enabled) {
        let payload = serde_json::from_str::<RouteCatalogPayload>(&route.catalog_json).ok();
        let model = payload
            .as_ref()
            .map(|payload| payload.model.clone())
            .or_else(|| serde_json::from_str::<CatalogModel>(&route.catalog_json).ok());
        if let Some(model) = model {
            targets.insert(
                model.slug.clone(),
                RouteTarget {
                    provider_id: route.provider_id.clone(),
                    upstream_model: route.upstream_model.clone(),
                    base_url: route.base_url.clone(),
                    protocol: route.protocol.clone(),
                },
            );
            models.push(model);
        }
    }
    models.sort_by(|left, right| left.slug.cmp(&right.slug));
    (ModelsResponse { models }, targets)
}

pub fn default_reasoning_levels() -> Vec<ReasoningEffortPreset> {
    ReasoningEffort::ALL
        .into_iter()
        .map(|effort| ReasoningEffortPreset {
            effort,
            description: format!("Codex {}", effort.as_str()),
        })
        .collect()
}

#[allow(dead_code)]
pub fn placeholder_model(slug: String, display_name: String) -> CatalogModel {
    CatalogModel {
        slug,
        display_name,
        description: Some("Codex Spur route".into()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: default_reasoning_levels(),
        shell_type: "default".into(),
        visibility: "list".into(),
        supported_in_api: true,
        priority: 0,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: String::new(),
        model_messages: None,
        include_skills_usage_instructions: false,
        supports_reasoning_summary_parameter: false,
        default_reasoning_summary: serde_json::Value::String("auto".into()),
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: Some("freeform".into()),
        web_search_tool_type: "text".into(),
        truncation_policy: TruncationPolicy {
            mode: "tokens".into(),
            limit: 0,
        },
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(128_000),
        max_context_window: Some(128_000),
        auto_compact_token_limit: Some(115_200),
        comp_hash: None,
        effective_context_window_percent: 90,
        experimental_supported_tools: vec!["shell".into(), "apply_patch".into()],
        input_modalities: vec!["text".into()],
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}
