use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig::{client::ProviderClient, providers::anthropic};

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_provider_settings::{
    CreateAnthropicProviderSettingsInput, LlmProviderSettings, create_anthropic_provider_settings,
};
use super::llm_task_runner::{RunLlmTaskInput, run_llm_task};
use crate::outbound::github::GitHubMcpConfig;

pub struct AnthropicTaskRunnerInput {
    pub api_key: String,
    pub model: String,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_mcp: GitHubMcpConfig,
    pub github_scope_org: String,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct AnthropicTaskRunner {
    api_key: String,
    provider_settings: LlmProviderSettings,
    datadog_mcp: DatadogMcpToolConfig,
    github_mcp: GitHubMcpConfig,
    github_scope_org: String,
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
            datadog_mcp: input.datadog_mcp,
            github_mcp: input.github_mcp,
            github_scope_org: input.github_scope_org,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for AnthropicTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_llm_task(RunLlmTaskInput {
            client: anthropic::Client::from_val(self.api_key.clone()),
            settings: self.provider_settings.clone(),
            datadog_mcp: self.datadog_mcp.clone(),
            github_mcp: self.github_mcp.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}
