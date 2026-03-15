use serde_json::{Value, json};

const DEFAULT_COORDINATOR_MAX_TURNS: usize = 20;
const DEFAULT_SPECIALIST_MAX_TURNS: usize = 50;
const DEFAULT_TOOL_CONCURRENCY: usize = 8;

#[derive(Debug, Clone)]
pub struct RigProviderSettings {
    pub provider: String,
    pub coordinator_model: String,
    pub specialist_model: String,
    pub coordinator_max_turns: usize,
    pub specialist_max_turns: usize,
    pub tool_concurrency: usize,
    pub additional_params: Value,
}

pub struct CreateOpenAiProviderSettingsInput {
    pub coordinator_model: String,
}

pub fn create_openai_provider_settings(
    input: CreateOpenAiProviderSettingsInput,
) -> RigProviderSettings {
    RigProviderSettings {
        provider: "openai".to_string(),
        specialist_model: input.coordinator_model.clone(),
        coordinator_model: input.coordinator_model,
        coordinator_max_turns: DEFAULT_COORDINATOR_MAX_TURNS,
        specialist_max_turns: DEFAULT_SPECIALIST_MAX_TURNS,
        tool_concurrency: DEFAULT_TOOL_CONCURRENCY,
        additional_params: json!({
            "reasoning": {
                "effort": "low",
                "summary": "auto",
            },
            "text": {
                "format": {
                    "type": "text",
                },
            },
            "parallel_tool_calls": true,
        }),
    }
}
