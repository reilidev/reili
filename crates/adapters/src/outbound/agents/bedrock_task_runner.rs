use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunReport, TaskRunnerPort};
use rig_bedrock::client::ClientBuilder;

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};
use super::llm_task_runner::{RunLlmTaskInput, run_llm_task};

pub struct BedrockTaskRunnerInput {
    pub region: String,
    pub model_id: String,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_scope_org: String,
    pub language: String,
}

pub struct BedrockTaskRunner {
    region: String,
    provider_settings: LlmProviderSettings,
    datadog_mcp: DatadogMcpToolConfig,
    github_scope_org: String,
    language: String,
}

impl BedrockTaskRunner {
    pub fn new(input: BedrockTaskRunnerInput) -> Self {
        Self {
            region: input.region,
            provider_settings: create_bedrock_provider_settings(
                CreateBedrockProviderSettingsInput {
                    model_id: input.model_id,
                },
            ),
            datadog_mcp: input.datadog_mcp,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for BedrockTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunReport, AgentRunFailedError> {
        let client = ClientBuilder::default().region(&self.region).build().await;

        run_llm_task(RunLlmTaskInput {
            client,
            settings: self.provider_settings.clone(),
            datadog_mcp: self.datadog_mcp.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            run: input,
        })
        .await
    }
}
