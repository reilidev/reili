use async_trait::async_trait;
use reili_core::cycle_agent::{CycleAgentOutcome, CycleAgentPort, RunCycleAgentInput};
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig_vertexai::Client;

use super::super::cycle_agent::{RunCycleAgentLlmInput, run_cycle_agent};
use super::super::provider_settings::{
    CreateVertexAiProviderSettingsInput, LlmProviderSettings, create_vertex_ai_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

pub struct VertexAiTaskRunnerInput {
    pub client: Client,
    pub model_id: String,
    pub sub_agent_model_id: String,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct VertexAiTaskRunner {
    client: Client,
    provider_settings: LlmProviderSettings,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl VertexAiTaskRunner {
    pub fn new(input: VertexAiTaskRunnerInput) -> Self {
        Self {
            client: input.client,
            provider_settings: create_vertex_ai_provider_settings(
                CreateVertexAiProviderSettingsInput {
                    model_id: input.model_id,
                    sub_agent_model_id: input.sub_agent_model_id,
                },
            ),
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for VertexAiTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_task(RunLlmTaskRunnerInput {
            client: self.client.clone(),
            sub_agent_client: self.client.clone(),
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}

#[async_trait]
impl CycleAgentPort for VertexAiTaskRunner {
    async fn run(
        &self,
        input: RunCycleAgentInput,
    ) -> Result<CycleAgentOutcome, AgentRunFailedError> {
        run_cycle_agent(RunCycleAgentLlmInput {
            client: self.client.clone(),
            sub_agent_client: self.client.clone(),
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}
