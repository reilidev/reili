use rig_vertexai::Client;

use super::super::llm_runner::{AgentClients, LlmBackend, LlmClients};
use super::super::provider_settings::{
    CreateVertexAiProviderSettingsInput, create_vertex_ai_provider_settings,
};

pub struct VertexAiBackendInput {
    pub client: Client,
    pub model_id: String,
    pub sub_agent_model_id: String,
}

#[must_use]
pub fn vertex_ai_backend(input: VertexAiBackendInput) -> LlmBackend {
    LlmBackend {
        clients: LlmClients::VertexAi(AgentClients {
            lead: input.client.clone(),
            sub_agent: input.client,
        }),
        settings: create_vertex_ai_provider_settings(CreateVertexAiProviderSettingsInput {
            model_id: input.model_id,
            sub_agent_model_id: input.sub_agent_model_id,
        }),
    }
}
