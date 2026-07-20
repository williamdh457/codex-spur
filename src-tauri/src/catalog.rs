use std::{collections::HashMap, sync::Arc};

use anyhow::{bail, Result};

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
    pub kind: String,
    pub upstream_model: String,
    pub base_url: String,
    pub protocol: String,
}

pub type SharedCatalog = Arc<RwLock<ModelsResponse>>;
pub type SharedRoutes = Arc<RwLock<HashMap<String, RouteTarget>>>;

const REQUIRED_MODEL_FIELDS: &[&str] = &[
    "slug",
    "display_name",
    "supported_reasoning_levels",
    "shell_type",
    "visibility",
    "supported_in_api",
    "priority",
    "additional_speed_tiers",
    "service_tiers",
    "availability_nux",
    "upgrade",
    "base_instructions",
    "include_skills_usage_instructions",
    "supports_reasoning_summaries",
    "supports_reasoning_summary_parameter",
    "default_reasoning_summary",
    "support_verbosity",
    "truncation_policy",
    "supports_parallel_tool_calls",
    "supports_image_detail_original",
    "effective_context_window_percent",
    "experimental_supported_tools",
    "input_modalities",
    "supports_search_tool",
    "use_responses_lite",
];

/// camelCase keys that historical SQLite rows used. Codex Desktop rejects them
/// (or fails to map them onto required snake_case ModelInfo fields), which
/// surfaces as an empty model picker after `Invalid configuration; using defaults`.
const FORBIDDEN_CAMEL_CASE_MARKERS: &[&str] = &[
    "\"displayName\"",
    "\"supportedReasoningLevels\"",
    "\"defaultReasoningLevel\"",
    "\"shellType\"",
    "\"supportedInApi\"",
    "\"experimentalSupportedTools\"",
    "\"baseInstructions\"",
    "\"inputModalities\"",
    "\"truncationPolicy\"",
    "\"applyPatchToolType\"",
    "\"webSearchToolType\"",
];

/// Validate the serialized surface that ChatGPT's bundled Codex parser consumes.
/// The local Rust type is intentionally permissive for healing old SQLite rows, so
/// this check must happen after serialization and before any file is replaced.
pub fn validate_catalog(catalog: &ModelsResponse) -> Result<()> {
    if catalog.models.is_empty() {
        bail!("catalog 至少需要一个模型");
    }

    let mut slugs = std::collections::HashSet::new();
    for (index, model) in catalog.models.iter().enumerate() {
        if model.slug.trim().is_empty() || model.slug.contains('/') {
            bail!("catalog 第 {} 个模型 slug 无效（禁止空值或含 /）", index + 1);
        }
        if !slugs.insert(&model.slug) {
            bail!("catalog 存在重复 slug：{}", model.slug);
        }
        if model.display_name.trim().is_empty()
            || model.shell_type != "shell_command"
            || model.visibility != "list"
            || model.base_instructions.trim().is_empty()
        {
            bail!(
                "catalog 第 {} 个模型缺少 Codex 基本字段（shell_type 必须为 shell_command，visibility=list，base_instructions 非空）",
                index + 1
            );
        }

        let efforts = model
            .supported_reasoning_levels
            .iter()
            .map(|level| level.effort)
            .collect::<Vec<_>>();
        if efforts != ReasoningEffort::ALL {
            bail!(
                "catalog 第 {} 个模型 reasoning levels 必须按 none, minimal, low, medium, high, xhigh, max, ultra 完整输出",
                index + 1
            );
        }
        if model
            .supported_reasoning_levels
            .iter()
            .any(|level| level.description.trim().is_empty())
        {
            bail!("catalog 第 {} 个模型存在空 reasoning 描述", index + 1);
        }
        let Some(default) = model.default_reasoning_level else {
            bail!("catalog 第 {} 个模型缺少 default_reasoning_level", index + 1);
        };
        if !efforts.contains(&default) {
            bail!("catalog 第 {} 个模型默认 reasoning 不在支持列表中", index + 1);
        }

        let value = serde_json::to_value(model)?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("catalog 第 {} 个模型不是 JSON object", index + 1))?;
        for field in REQUIRED_MODEL_FIELDS {
            if !object.contains_key(*field) {
                bail!(
                    "catalog 第 {} 个模型缺少字段 {}（Codex 会 Invalid configuration 并清空模型列表）",
                    index + 1,
                    field
                );
            }
        }

        // All custom-provider catalog rows must stay lean — including GPT-named ones.
        // Working CC Switch catalogs keep experimental_supported_tools: [] for
        // gpt-5.6-terra/sol/luna. Advertising shell/apply_patch has emptied Desktop's
        // bottom-right model list in the field.
        if !model.experimental_supported_tools.is_empty() {
            bail!(
                "catalog 第 {} 个模型不得广告 experimental_supported_tools（须为空数组，对齐 CC Switch）",
                index + 1
            );
        }
        if model.apply_patch_tool_type.is_some() || model.web_search_tool_type.is_some() {
            bail!(
                "catalog 第 {} 个模型不得设置 apply_patch/web_search tool type",
                index + 1
            );
        }
    }

    // Whole-document camelCase scan (catches accidental re-serialize regressions).
    let raw = serde_json::to_string(catalog)?;
    for marker in FORBIDDEN_CAMEL_CASE_MARKERS {
        if raw.contains(marker) {
            bail!(
                "catalog JSON 含 camelCase 字段 {}；Codex Desktop 要求 snake_case，会解析失败并清空模型列表",
                marker
            );
        }
    }
    if !raw.contains("\"experimental_supported_tools\"") {
        bail!("catalog JSON 缺少 experimental_supported_tools（Codex 硬依赖）");
    }
    Ok(())
}

/// Re-heal a single SQLite `catalog_json` blob into the snake_case RouteCatalogPayload
/// shape that `apply` and the proxy both expect. Used to scrub camelCase legacy rows.
pub fn heal_stored_catalog_json(route: &StoredRoute) -> Result<String> {
    let mut model = match serde_json::from_str::<RouteCatalogPayload>(&route.catalog_json) {
        Ok(payload) => payload.model,
        Err(_) => serde_json::from_str::<CatalogModel>(&route.catalog_json).map_err(|error| {
            anyhow::anyhow!(
                "模型路由 {} 的 catalog_json 无法解析：{}",
                route.id,
                error
            )
        })?,
    };
    crate::providers::normalize_catalog_model_for_codex_with_kind(
        &mut model,
        1000,
        Some(route.kind.as_str()),
    );
    // Prefer Desktop-native public slug when unique; heal path claims freely (single row).
    let mut claimed = std::collections::HashSet::new();
    model.slug = crate::providers::catalog_publish_slug(
        &route.provider_id,
        &route.upstream_model,
        &mut claimed,
    );
    if model.display_name.trim().is_empty() {
        model.display_name = route.display_name.clone();
    }
    let reasoning_profile = crate::providers::reasoning_profile(&route.kind, &route.upstream_model);
    let payload = RouteCatalogPayload {
        model,
        reasoning_profile,
    };
    Ok(serde_json::to_string(&payload)?)
}

/// Canonical short labels for Desktop-native model ids (terra / sol / luna).
fn desktop_native_short_label(native_slug: &str) -> &str {
    match native_slug {
        "gpt-5.6-terra" => "GPT-5.6-Terra",
        "gpt-5.6-sol" => "GPT-5.6-Sol",
        "gpt-5.6-luna" => "GPT-5.6-Luna",
        other => other,
    }
}

/// Strip any existing `Prefix · ` so we can re-apply the current instance name.
fn bare_model_label(display: &str) -> &str {
    display
        .rsplit_once('·')
        .map(|(_, rest)| rest.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| display.trim())
}

/// DESIGN.md default: every Codex picker row is `供应商 · 模型`.
fn format_catalog_display_name(provider_name: &str, model_label: &str) -> String {
    let provider = provider_name.trim();
    let bare = bare_model_label(model_label);
    if provider.is_empty() {
        return bare.to_string();
    }
    if bare.is_empty() {
        return provider.to_string();
    }
    format!("{provider} · {bare}")
}

/// Prefer native short names; otherwise bare label from catalog / route / upstream id.
fn catalog_model_label(route: &StoredRoute, model: &CatalogModel) -> String {
    if let Some(native) = crate::providers::desktop_native_model_slug(&route.upstream_model) {
        return desktop_native_short_label(native).to_string();
    }
    let from_model = model.display_name.trim();
    let from_route = route.display_name.trim();
    let raw = if !from_model.is_empty() {
        from_model
    } else if !from_route.is_empty() {
        from_route
    } else {
        route.upstream_model.as_str()
    };
    bare_model_label(raw).to_string()
}

pub fn build_from_routes(
    routes: &[StoredRoute],
) -> Result<(ModelsResponse, HashMap<String, RouteTarget>)> {
    let mut models = Vec::new();
    let mut targets = HashMap::new();
    let mut claimed_public_slugs = std::collections::HashSet::new();
    for (enabled_index, route) in routes.iter().filter(|route| route.enabled).enumerate() {
        let enabled_index = enabled_index as i32;
        let model = match serde_json::from_str::<RouteCatalogPayload>(&route.catalog_json) {
            Ok(payload) => payload.model,
            Err(payload_error) => {
                serde_json::from_str::<CatalogModel>(&route.catalog_json).map_err(|model_error| {
                    anyhow::anyhow!(
                        "模型路由 {} 的 catalog_json 无法解析：{}；{}",
                        route.id,
                        payload_error,
                        model_error
                    )
                })?
            }
        };
        {
            let mut model = model;
            // Heal stale SQLite rows (camelCase era, technical effort copy, weak meta)
            // so ChatGPT always receives a Nice/CC Switch–compatible catalog shape.
            crate::providers::normalize_catalog_model_for_codex_with_kind(
                &mut model,
                1000 + enabled_index,
                Some(route.kind.as_str()),
            );
            // Desktop power picker only lists gpt-5.6-terra/sol (CC Switch style). Prefer
            // those public slugs when unique; otherwise keep opaque spur-route-*.
            let opaque = crate::providers::opaque_route_slug(
                &route.provider_id,
                &route.upstream_model,
            );
            let published = crate::providers::catalog_publish_slug(
                &route.provider_id,
                &route.upstream_model,
                &mut claimed_public_slugs,
            );
            let legacy = crate::providers::legacy_route_slug(
                &route.provider_id,
                &route.upstream_model,
            );
            let previous_slug = model.slug.clone();
            model.slug = published.clone();
            // Always publish "供应商 · 模型" — including OpenAI official subscription
            // and account-pool instances that previously lost their prefix for native
            // terra/sol/luna public slugs.
            let label =
                format_catalog_display_name(&route.provider_name, &catalog_model_label(route, &model));
            model.display_name = label.clone();
            model.description = Some(label);
            let base_url = if route.kind.eq_ignore_ascii_case("xai") {
                // Runtime resolve so OAuth subscription never sticks on api.x.ai
                // even if a row was not yet migrated.
                crate::providers::resolve_xai_upstream_base(
                    route.entry_category.as_deref(),
                    Some(route.base_url.as_str()),
                )
            } else {
                route.base_url.clone()
            };
            let target = RouteTarget {
                provider_id: route.provider_id.clone(),
                kind: route.kind.clone(),
                upstream_model: route.upstream_model.clone(),
                base_url,
                protocol: route.protocol.clone(),
            };
            // Publish key + dual-keys so in-flight sessions on old slugs still route.
            targets.insert(published.clone(), target.clone());
            if opaque != published {
                targets.insert(opaque, target.clone());
            }
            if legacy != published {
                targets.insert(legacy, target.clone());
            }
            if route.id != published {
                targets.insert(route.id.clone(), target.clone());
            }
            if previous_slug != published && !previous_slug.is_empty() {
                targets.insert(previous_slug, target);
            }
            models.push(model);
        }
    }
    // Prefer human display order: non-GPT / third-party first, then name — helps GUI scanning.
    models.sort_by(|left, right| {
        let left_gpt = left.display_name.to_ascii_lowercase().contains("gpt");
        let right_gpt = right.display_name.to_ascii_lowercase().contains("gpt");
        left_gpt
            .cmp(&right_gpt)
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.slug.cmp(&right.slug))
    });
    // Re-assign priorities after stable sort so ordering is deterministic.
    for (index, model) in models.iter_mut().enumerate() {
        model.priority = 1000 + index as i32;
    }
    let catalog = ModelsResponse { models };
    if !catalog.models.is_empty() {
        validate_catalog(&catalog)?;
    }
    Ok((catalog, targets))
}

pub fn default_reasoning_levels() -> Vec<ReasoningEffortPreset> {
    ReasoningEffort::ALL
        .into_iter()
        .map(|effort| ReasoningEffortPreset {
            effort,
            description: crate::providers::codex_reasoning_level_description(effort).into(),
        })
        .collect()
}

#[allow(dead_code)]
pub fn placeholder_model(slug: String, display_name: String) -> CatalogModel {
    let mut model = CatalogModel {
        slug,
        display_name,
        description: Some("Codex Spur route".into()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: default_reasoning_levels(),
        shell_type: "shell_command".into(),
        visibility: "list".into(),
        supported_in_api: true,
        priority: 1000,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: crate::providers::CODEX_AGENT_BASE_INSTRUCTIONS.into(),
        model_messages: None,
        include_skills_usage_instructions: false,
        supports_reasoning_summaries: true,
        supports_reasoning_summary_parameter: false,
        default_reasoning_summary: serde_json::Value::String("auto".into()),
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: None,
        truncation_policy: TruncationPolicy {
            mode: "bytes".into(),
            limit: 10_000,
        },
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: Some(128_000),
        max_context_window: Some(128_000),
        auto_compact_token_limit: Some(115_200),
        comp_hash: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec!["text".into()],
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    };
    crate::providers::normalize_catalog_model_for_codex(&mut model, 1000);
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{legacy_route_slug, opaque_route_slug};
    use crate::storage::StoredRoute;

    fn stale_slash_route(
        provider_id: &str,
        provider_name: &str,
        upstream: &str,
        display: &str,
        kind: &str,
    ) -> StoredRoute {
        let legacy = legacy_route_slug(provider_id, upstream);
        // Intentionally stale: slash slug + camelCase + truncated ladder (what used to break GUI).
        let catalog_json = serde_json::json!({
            "model": {
                "slug": legacy,
                "displayName": display,
                "description": "stale",
                "defaultReasoningLevel": "high",
                "supportedReasoningLevels": [
                    {"effort": "none", "description": "Disable Thinking"},
                    {"effort": "high", "description": "Enabled Thinking"}
                ],
                "shellType": "shell_command",
                "visibility": "list",
                "supportedInApi": true,
                "priority": 0
            },
            "reasoning_profile": {
                "title": "stale",
                "mappings": []
            }
        })
        .to_string();
        StoredRoute {
            id: legacy,
            provider_id: provider_id.into(),
            provider_name: provider_name.into(),
            kind: kind.into(),
            upstream_model: upstream.into(),
            display_name: display.into(),
            enabled: true,
            catalog_json,
            protocol: "chat_completions".into(),
            base_url: "https://example.invalid/v1".into(),
            entry_category: None,
        }
    }

    #[test]
    fn format_catalog_display_name_always_prefixes_provider() {
        assert_eq!(
            format_catalog_display_name("OpenAI", "GPT-5.6-Sol"),
            "OpenAI · GPT-5.6-Sol"
        );
        assert_eq!(
            format_catalog_display_name("账号池", "OpenAI · GPT-5.6-Sol"),
            "账号池 · GPT-5.6-Sol"
        );
        assert_eq!(
            format_catalog_display_name("Kimi tian", "K3"),
            "Kimi tian · K3"
        );
    }

    #[test]
    fn build_from_routes_heals_slash_slugs_and_full_ladder_for_codex_gui() {
        let routes = vec![
            stale_slash_route(
                "c99e00b6-b386-4980-af2c-8b4be927e34a",
                "OpenAI 2",
                "gpt-5.6-luna",
                "OpenAI 2 · GPT-5.6-Luna",
                "openai",
            ),
            stale_slash_route(
                "d643a92a-76ae-458a-a567-81c0ea171ea5",
                "Kimi tian",
                "k3",
                "Kimi tian · K3",
                "kimi",
            ),
        ];
        let (catalog, targets) = build_from_routes(&routes).expect("build catalog");
        assert_eq!(catalog.models.len(), 2);

        // Kimi should sort before GPT for picker scanning.
        assert!(
            catalog.models[0].display_name.to_ascii_lowercase().contains("kimi")
                || !catalog.models[0]
                    .display_name
                    .to_ascii_lowercase()
                    .contains("gpt")
        );

        for model in &catalog.models {
            assert!(!model.slug.contains('/'), "slug must not contain /, got {}", model.slug);
            let efforts: Vec<_> = model
                .supported_reasoning_levels
                .iter()
                .map(|level| level.effort.as_str())
                .collect();
            assert_eq!(
                efforts,
                vec![
                    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"
                ]
            );
            assert!(targets.contains_key(&model.slug));
            // Custom-provider rows never advertise shell/apply_patch (CC Switch shape).
            assert!(model.experimental_supported_tools.is_empty());
            assert!(model.apply_patch_tool_type.is_none());
            // DESIGN.md: every picker row is "供应商 · 模型".
            assert!(
                model.display_name.contains('·'),
                "display_name must keep provider prefix, got {}",
                model.display_name
            );
        }

        // OpenAI luna: public slug for power picker + instance-prefixed display name.
        let luna = catalog
            .models
            .iter()
            .find(|m| m.slug == "gpt-5.6-luna" || m.display_name.contains("Luna"))
            .expect("luna model");
        assert_eq!(luna.slug, "gpt-5.6-luna");
        assert_eq!(luna.display_name, "OpenAI 2 · GPT-5.6-Luna");
        assert_eq!(luna.description.as_deref(), Some("OpenAI 2 · GPT-5.6-Luna"));

        // Kimi stays opaque (not a Desktop power-picker preset) and keeps its prefix.
        let kimi = catalog
            .models
            .iter()
            .find(|m| m.display_name.contains("Kimi"))
            .expect("kimi model");
        assert_eq!(kimi.display_name, "Kimi tian · K3");
        assert!(
            kimi.slug.starts_with("spur-route-"),
            "kimi slug must stay opaque, got {}",
            kimi.slug
        );

        // Dual-key: legacy uuid/upstream still routes to same upstream.
        let legacy_kimi = legacy_route_slug("d643a92a-76ae-458a-a567-81c0ea171ea5", "k3");
        let opaque_kimi = opaque_route_slug("d643a92a-76ae-458a-a567-81c0ea171ea5", "k3");
        assert_ne!(legacy_kimi, opaque_kimi);
        assert_eq!(
            targets.get(&legacy_kimi).map(|t| t.upstream_model.as_str()),
            Some("k3")
        );
        assert_eq!(
            targets.get(&opaque_kimi).map(|t| t.upstream_model.as_str()),
            Some("k3")
        );
        // Public luna slug + opaque alias both route.
        assert_eq!(
            targets.get("gpt-5.6-luna").map(|t| t.upstream_model.as_str()),
            Some("gpt-5.6-luna")
        );
        let opaque_luna = opaque_route_slug("c99e00b6-b386-4980-af2c-8b4be927e34a", "gpt-5.6-luna");
        assert_eq!(
            targets.get(&opaque_luna).map(|t| t.upstream_model.as_str()),
            Some("gpt-5.6-luna")
        );

        // Published JSON must keep required fields so Codex CLI can parse.
        let raw = serde_json::to_string_pretty(&catalog).expect("serialize catalog");
        assert!(raw.contains("\"support_verbosity\""));
        // Empty array still required (Codex rejects missing experimental_supported_tools).
        assert!(raw.contains("\"experimental_supported_tools\""));
        assert!(!raw.contains("\"displayName\""));
        assert!(!raw.contains("\"supportedReasoningLevels\""));
        assert!(raw.contains("\"additional_speed_tiers\": []"));
        assert!(raw.contains("\"service_tiers\": []"));
        assert!(raw.contains("\"availability_nux\": null"));
        assert!(raw.contains("\"upgrade\": null"));
        assert!(raw.contains("\"effort\": \"max\""));
        assert!(raw.contains("Kimi tian · K3"));
        assert!(raw.contains("OpenAI 2 · GPT-5.6-Luna"));
        assert!(raw.contains("\"gpt-5.6-luna\""));
    }

    #[test]
    fn build_from_routes_keeps_provider_prefix_for_native_openai_and_pool() {
        // Official subscription instance + account-pool instance both share gpt-5.6-sol;
        // first claims public slug, second stays opaque — both must keep instance prefix.
        let routes = vec![
            stale_slash_route(
                "official-instance",
                "官方订阅",
                "gpt-5.6-sol",
                "GPT-5.6-Sol", // bare label in stale row — assemble must re-prefix
                "openai",
            ),
            stale_slash_route(
                "pool-instance",
                "账号池",
                "gpt-5.6-sol",
                "账号池 · GPT-5.6-Sol",
                "openai",
            ),
        ];
        let (catalog, targets) = build_from_routes(&routes).expect("build catalog");
        assert_eq!(catalog.models.len(), 2);

        let official = catalog
            .models
            .iter()
            .find(|m| m.display_name.starts_with("官方订阅"))
            .expect("official");
        let pool = catalog
            .models
            .iter()
            .find(|m| m.display_name.starts_with("账号池"))
            .expect("pool");

        assert_eq!(official.display_name, "官方订阅 · GPT-5.6-Sol");
        assert_eq!(pool.display_name, "账号池 · GPT-5.6-Sol");
        assert_eq!(official.slug, "gpt-5.6-sol");
        assert!(
            pool.slug.starts_with("spur-route-"),
            "second sol claim stays opaque, got {}",
            pool.slug
        );
        assert!(targets.contains_key("gpt-5.6-sol"));
        assert!(targets.contains_key(&pool.slug));
    }

    #[test]
    fn strict_validation_checks_every_model_and_required_empty_arrays() {
        let mut first = placeholder_model("spur-route-first".into(), "First".into());
        let mut second = placeholder_model("spur-route-second".into(), "Second".into());
        first.experimental_supported_tools.clear();
        validate_catalog(&ModelsResponse {
            models: vec![first, second.clone()],
        })
        .expect("empty required array must serialize and validate");

        second.base_instructions.clear();
        let error = validate_catalog(&ModelsResponse {
            models: vec![
                placeholder_model("spur-route-first".into(), "First".into()),
                second,
            ],
        })
        .unwrap_err();
        assert!(error.to_string().contains("第 2 个模型"));
    }

    #[test]
    fn strict_validation_rejects_incomplete_reasoning_ladder() {
        let mut model = placeholder_model("spur-route-kimi".into(), "Kimi".into());
        model
            .supported_reasoning_levels
            .retain(|level| level.effort != ReasoningEffort::Minimal);
        let error = validate_catalog(&ModelsResponse { models: vec![model] }).unwrap_err();
        assert!(error.to_string().contains("minimal"));
    }

    #[test]
    fn build_from_routes_rejects_malformed_enabled_rows() {
        let mut route = stale_slash_route("provider", "Kimi", "k3", "Kimi", "kimi");
        route.catalog_json = "{not-json".into();
        let error = build_from_routes(&[route]).unwrap_err();
        assert!(error.to_string().contains("catalog_json 无法解析"));
    }

    #[test]
    fn heal_stored_catalog_json_rewrites_camel_case_to_snake_case() {
        let route = stale_slash_route(
            "d643a92a-76ae-458a-a567-81c0ea171ea5",
            "Kimi tian",
            "k3",
            "Kimi tian · K3",
            "kimi",
        );
        assert!(route.catalog_json.contains("displayName"));
        let healed = heal_stored_catalog_json(&route).expect("heal");
        assert!(healed.contains("\"display_name\""));
        assert!(!healed.contains("\"displayName\""));
        assert!(healed.contains("\"experimental_supported_tools\""));
        assert!(!healed.contains("\"experimentalSupportedTools\""));
        assert!(healed.contains("spur-route-"));
        // Third-party rows must not advertise GPT-native tools after heal.
        let payload: RouteCatalogPayload = serde_json::from_str(&healed).expect("payload");
        assert!(payload.model.experimental_supported_tools.is_empty());
        assert!(payload.model.apply_patch_tool_type.is_none());
        assert!(payload.model.web_search_tool_type.is_none());
        assert_eq!(payload.model.shell_type, "shell_command");
    }

    #[test]
    fn validate_catalog_rejects_third_party_tool_ads() {
        let mut model = placeholder_model("spur-route-kimi".into(), "Kimi · K3".into());
        model.experimental_supported_tools = vec!["shell".into()];
        let error = validate_catalog(&ModelsResponse {
            models: vec![model],
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("experimental_supported_tools")
                || error.to_string().contains("不得广告")
        );
    }
}
