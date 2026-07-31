use crate::domain::ProviderSummary;

pub fn provider_presets() -> Vec<ProviderSummary> {
    vec![
        ProviderSummary {
            id: "kimi".into(),
            name: "Kimi".into(),
            region: "中国 / Global".into(),
            protocol: "Responses preferred".into(),
            configured: false,
            selected_models: 0,
            discovered_models: 0,
            last_fetched_at: None,
        },
        ProviderSummary {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            region: "Global".into(),
            // Official Codex script (2026-07-31): wire_api = "responses" for V4 Flash.
            protocol: "Responses".into(),
            configured: false,
            selected_models: 0,
            discovered_models: 0,
            last_fetched_at: None,
        },
        ProviderSummary {
            id: "minimax".into(),
            name: "MiniMax".into(),
            region: "中国 / Global".into(),
            protocol: "Responses preferred".into(),
            configured: false,
            selected_models: 0,
            discovered_models: 0,
            last_fetched_at: None,
        },
    ]
}
