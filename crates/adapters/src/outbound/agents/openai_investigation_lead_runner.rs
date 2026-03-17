use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    InvestigationLeadRunReport, InvestigationLeadRunnerPort, RunInvestigationLeadInput,
};
use rig::{client::ProviderClient, providers::openai};

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_investigation_lead_runner::{
    RunLlmInvestigationLeadInput, run_llm_investigation_lead,
};
use super::llm_provider_settings::{
    CreateOpenAiProviderSettingsInput, LlmProviderSettings, create_openai_provider_settings,
};

pub struct OpenAiInvestigationLeadRunnerInput {
    pub api_key: String,
    pub investigation_lead_model: String,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_scope_org: String,
    pub language: String,
}

pub struct OpenAiInvestigationLeadRunner {
    api_key: String,
    provider_settings: LlmProviderSettings,
    datadog_mcp: DatadogMcpToolConfig,
    github_scope_org: String,
    language: String,
}

impl OpenAiInvestigationLeadRunner {
    pub fn new(input: OpenAiInvestigationLeadRunnerInput) -> Self {
        Self {
            api_key: input.api_key,
            provider_settings: create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
                investigation_lead_model: input.investigation_lead_model,
            }),
            datadog_mcp: input.datadog_mcp,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl InvestigationLeadRunnerPort for OpenAiInvestigationLeadRunner {
    async fn run(
        &self,
        input: RunInvestigationLeadInput,
    ) -> Result<InvestigationLeadRunReport, AgentRunFailedError> {
        run_llm_investigation_lead(RunLlmInvestigationLeadInput {
            client: openai::Client::from_val(self.api_key.clone().into()),
            settings: self.provider_settings.clone(),
            datadog_mcp: self.datadog_mcp.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            run: input,
        })
        .await
    }
}
