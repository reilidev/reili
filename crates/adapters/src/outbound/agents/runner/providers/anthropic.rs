use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::secret::SecretString;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig::{client::ProviderClient, providers::anthropic};

use super::super::provider_settings::{
    CreateAnthropicProviderSettingsInput, LlmProviderSettings, create_anthropic_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

pub struct AnthropicTaskRunnerInput {
    pub api_key: SecretString,
    pub model: String,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct AnthropicTaskRunner {
    api_key: SecretString,
    provider_settings: LlmProviderSettings,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl AnthropicTaskRunner {
    pub fn new(input: AnthropicTaskRunnerInput) -> Self {
        Self {
            api_key: input.api_key,
            provider_settings: create_anthropic_provider_settings(
                CreateAnthropicProviderSettingsInput { model: input.model },
            ),
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for AnthropicTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_task(RunLlmTaskRunnerInput {
            client: anthropic::Client::from_val(self.api_key.expose().to_string()),
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}
