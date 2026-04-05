use serde_json::{Value, json};

const DEFAULT_TASK_RUNNER_MAX_TURNS: usize = 20;
const DEFAULT_SPECIALIST_MAX_TURNS: usize = 50;
const DEFAULT_TOOL_CONCURRENCY: usize = 8;
const ANTHROPIC_OPUS_4_6_MODEL: &str = "claude-opus-4-6";
const ANTHROPIC_SONNET_4_6_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_HAIKU_4_5_MODEL: &str = "claude-haiku-4-5";

#[derive(Debug, Clone)]
pub struct LlmProviderSettings {
    pub provider: String,
    pub task_runner_model: String,
    pub specialist_model: String,
    pub task_runner_max_turns: usize,
    pub specialist_max_turns: usize,
    pub tool_concurrency: usize,
    pub max_tokens: Option<u64>,
    pub additional_params: Value,
}

pub struct CreateOpenAiProviderSettingsInput {
    pub task_runner_model: String,
}

pub struct CreateAnthropicProviderSettingsInput {
    pub model: String,
}

pub struct CreateBedrockProviderSettingsInput {
    pub model_id: String,
}

pub struct CreateVertexAiProviderSettingsInput {
    pub model_id: String,
}

pub fn create_openai_provider_settings(
    input: CreateOpenAiProviderSettingsInput,
) -> LlmProviderSettings {
    LlmProviderSettings {
        provider: "openai".to_string(),
        specialist_model: input.task_runner_model.clone(),
        task_runner_model: input.task_runner_model,
        task_runner_max_turns: DEFAULT_TASK_RUNNER_MAX_TURNS,
        specialist_max_turns: DEFAULT_SPECIALIST_MAX_TURNS,
        tool_concurrency: DEFAULT_TOOL_CONCURRENCY,
        max_tokens: None,
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

pub fn create_anthropic_provider_settings(
    input: CreateAnthropicProviderSettingsInput,
) -> LlmProviderSettings {
    let max_tokens = anthropic_max_tokens(&input.model);

    LlmProviderSettings {
        provider: "anthropic".to_string(),
        specialist_model: input.model.clone(),
        task_runner_model: input.model,
        task_runner_max_turns: DEFAULT_TASK_RUNNER_MAX_TURNS,
        specialist_max_turns: DEFAULT_SPECIALIST_MAX_TURNS,
        tool_concurrency: DEFAULT_TOOL_CONCURRENCY,
        max_tokens,
        additional_params: json!({}),
    }
}

pub fn create_bedrock_provider_settings(
    input: CreateBedrockProviderSettingsInput,
) -> LlmProviderSettings {
    LlmProviderSettings {
        provider: "bedrock".to_string(),
        specialist_model: input.model_id.clone(),
        task_runner_model: input.model_id,
        task_runner_max_turns: DEFAULT_TASK_RUNNER_MAX_TURNS,
        specialist_max_turns: DEFAULT_SPECIALIST_MAX_TURNS,
        tool_concurrency: DEFAULT_TOOL_CONCURRENCY,
        max_tokens: None,
        additional_params: json!({}),
    }
}

pub fn create_vertex_ai_provider_settings(
    input: CreateVertexAiProviderSettingsInput,
) -> LlmProviderSettings {
    LlmProviderSettings {
        provider: "vertexai".to_string(),
        specialist_model: input.model_id.clone(),
        task_runner_model: input.model_id,
        task_runner_max_turns: DEFAULT_TASK_RUNNER_MAX_TURNS,
        specialist_max_turns: DEFAULT_SPECIALIST_MAX_TURNS,
        tool_concurrency: DEFAULT_TOOL_CONCURRENCY,
        max_tokens: None,
        additional_params: json!({}),
    }
}

fn anthropic_max_tokens(model: &str) -> Option<u64> {
    match model {
        ANTHROPIC_OPUS_4_6_MODEL => Some(32_000),
        ANTHROPIC_SONNET_4_6_MODEL => Some(64_000),
        ANTHROPIC_HAIKU_4_5_MODEL => Some(4_096),
        _ => None,
    }
}
