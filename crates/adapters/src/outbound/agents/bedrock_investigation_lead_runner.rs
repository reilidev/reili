use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    InvestigationLeadRunReport, InvestigationLeadRunnerPort, RunInvestigationLeadInput,
};
use rig_bedrock::client::ClientBuilder;

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_investigation_lead_runner::{
    RunLlmInvestigationLeadInput, run_llm_investigation_lead,
};
use super::llm_provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};

pub struct BedrockInvestigationLeadRunnerInput {
    pub region: String,
    pub model_id: String,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_scope_org: String,
    pub language: String,
}

pub struct BedrockInvestigationLeadRunner {
    region: String,
    provider_settings: LlmProviderSettings,
    datadog_mcp: DatadogMcpToolConfig,
    github_scope_org: String,
    language: String,
}

impl BedrockInvestigationLeadRunner {
    pub fn new(input: BedrockInvestigationLeadRunnerInput) -> Self {
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
impl InvestigationLeadRunnerPort for BedrockInvestigationLeadRunner {
    async fn run(
        &self,
        input: RunInvestigationLeadInput,
    ) -> Result<InvestigationLeadRunReport, AgentRunFailedError> {
        let client = ClientBuilder::default().region(&self.region).build().await;

        run_llm_investigation_lead(RunLlmInvestigationLeadInput {
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
