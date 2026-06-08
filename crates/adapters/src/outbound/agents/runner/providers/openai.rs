use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::secret::SecretString;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig::{client::ProviderClient, providers::openai};

use super::super::provider_settings::{
    CreateOpenAiProviderSettingsInput, LlmProviderSettings, create_openai_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

pub struct OpenAiTaskRunnerInput {
    pub api_key: SecretString,
    pub model: String,
    pub reasoning_effort: String,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct OpenAiTaskRunner {
    api_key: SecretString,
    provider_settings: LlmProviderSettings,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl OpenAiTaskRunner {
    pub fn new(input: OpenAiTaskRunnerInput) -> Self {
        Self {
            api_key: input.api_key,
            provider_settings: create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
                model: input.model,
                reasoning_effort: input.reasoning_effort,
            }),
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for OpenAiTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_task(RunLlmTaskRunnerInput {
            client: openai::Client::from_val(self.api_key.expose().to_string().into()),
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}
