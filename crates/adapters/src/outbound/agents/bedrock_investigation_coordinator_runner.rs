use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    CoordinatorRunReport, InvestigationCoordinatorRunnerPort, RunCoordinatorInput,
};
use rig_bedrock::client::ClientBuilder;

use super::llm_coordinator_runner::{RunLlmCoordinatorInput, run_llm_coordinator};
use super::llm_provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};

pub struct BedrockInvestigationCoordinatorRunnerInput {
    pub region: String,
    pub model_id: String,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub language: String,
}

pub struct BedrockInvestigationCoordinatorRunner {
    region: String,
    provider_settings: LlmProviderSettings,
    datadog_site: String,
    github_scope_org: String,
    language: String,
}

impl BedrockInvestigationCoordinatorRunner {
    pub fn new(input: BedrockInvestigationCoordinatorRunnerInput) -> Self {
        Self {
            region: input.region,
            provider_settings: create_bedrock_provider_settings(
                CreateBedrockProviderSettingsInput {
                    model_id: input.model_id,
                },
            ),
            datadog_site: input.datadog_site,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl InvestigationCoordinatorRunnerPort for BedrockInvestigationCoordinatorRunner {
    async fn run(
        &self,
        input: RunCoordinatorInput,
    ) -> Result<CoordinatorRunReport, AgentRunFailedError> {
        let client = ClientBuilder::default().region(&self.region).build().await;

        run_llm_coordinator(RunLlmCoordinatorInput {
            client,
            settings: self.provider_settings.clone(),
            datadog_site: self.datadog_site.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            run: input,
        })
        .await
    }
}
