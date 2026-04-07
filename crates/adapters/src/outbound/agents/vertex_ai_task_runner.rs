use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig_vertexai::Client;

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_provider_settings::{
    CreateVertexAiProviderSettingsInput, LlmProviderSettings, create_vertex_ai_provider_settings,
};
use super::llm_task_runner::{RunLlmTaskInput, run_llm_task};
use crate::outbound::github::GitHubMcpConfig;

pub struct VertexAiTaskRunnerInput {
    pub client: Client,
    pub model_id: String,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_mcp: GitHubMcpConfig,
    pub github_scope_org: String,
    pub language: String,
}

pub struct VertexAiTaskRunner {
    client: Client,
    provider_settings: LlmProviderSettings,
    datadog_mcp: DatadogMcpToolConfig,
    github_mcp: GitHubMcpConfig,
    github_scope_org: String,
    language: String,
}

impl VertexAiTaskRunner {
    pub fn new(input: VertexAiTaskRunnerInput) -> Self {
        Self {
            client: input.client,
            provider_settings: create_vertex_ai_provider_settings(
                CreateVertexAiProviderSettingsInput {
                    model_id: input.model_id,
                },
            ),
            datadog_mcp: input.datadog_mcp,
            github_mcp: input.github_mcp,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for VertexAiTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_llm_task(RunLlmTaskInput {
            client: self.client.clone(),
            settings: self.provider_settings.clone(),
            datadog_mcp: self.datadog_mcp.clone(),
            github_mcp: self.github_mcp.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            run: input,
        })
        .await
    }
}
