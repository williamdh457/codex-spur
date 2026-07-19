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
            protocol: "Chat Completions".into(),
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
