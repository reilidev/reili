use reili_core::error::PortError;
use reili_core::secret::SecretString;
use rig::{client::ProviderClient, providers::openai};

use super::super::llm_runner::{AgentClients, LlmBackend, LlmClients};
use super::super::provider_settings::{
    CreateOpenAiProviderSettingsInput, create_openai_provider_settings,
};

pub struct OpenAiBackendInput {
    pub api_key: SecretString,
    pub model: String,
    pub sub_agent_model: String,
    pub reasoning_effort: String,
}

pub fn openai_backend(input: OpenAiBackendInput) -> Result<LlmBackend, PortError> {
    let client = openai::Client::from_val(input.api_key.expose().to_string().into())
        .map_err(|error| PortError::new(format!("Failed to build OpenAI client: {error}")))?;
    Ok(LlmBackend {
        clients: LlmClients::OpenAi(AgentClients {
            lead: client.clone(),
            sub_agent: client,
        }),
        settings: create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
            model: input.model,
            sub_agent_model: input.sub_agent_model,
            reasoning_effort: input.reasoning_effort,
        }),
    })
}
