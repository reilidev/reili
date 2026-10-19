use reili_core::error::PortError;
use reili_core::secret::SecretString;
use rig::{client::ProviderClient, providers::anthropic};

use super::super::llm_runner::{AgentClients, LlmBackend, LlmClients};
use super::super::provider_settings::{
    CreateAnthropicProviderSettingsInput, create_anthropic_provider_settings,
};

pub struct AnthropicBackendInput {
    pub api_key: SecretString,
    pub model: String,
    pub sub_agent_model: String,
}

pub fn anthropic_backend(input: AnthropicBackendInput) -> Result<LlmBackend, PortError> {
    let client = anthropic::Client::from_val(input.api_key.expose().to_string())
        .map_err(|error| PortError::new(format!("Failed to build Anthropic client: {error}")))?;
    Ok(LlmBackend {
        clients: LlmClients::Anthropic(AgentClients {
            lead: client.clone(),
            sub_agent: client,
        }),
        settings: create_anthropic_provider_settings(CreateAnthropicProviderSettingsInput {
            model: input.model,
            sub_agent_model: input.sub_agent_model,
        }),
    })
}
