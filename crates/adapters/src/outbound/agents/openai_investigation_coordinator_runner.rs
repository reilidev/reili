use async_trait::async_trait;
use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    CoordinatorRunReport, InvestigationCoordinatorRunnerPort, RunCoordinatorInput,
};
use rig::{client::ProviderClient, providers::openai};

use super::llm_coordinator_runner::{RunLlmCoordinatorInput, run_llm_coordinator};
use super::provider_settings::{
    CreateOpenAiProviderSettingsInput, RigProviderSettings, create_openai_provider_settings,
};

pub struct OpenAiInvestigationCoordinatorRunnerInput {
    pub api_key: String,
    pub coordinator_model: String,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub language: String,
}

pub struct OpenAiInvestigationCoordinatorRunner {
    api_key: String,
    provider_settings: RigProviderSettings,
    datadog_site: String,
    github_scope_org: String,
    language: String,
}

impl OpenAiInvestigationCoordinatorRunner {
    pub fn new(input: OpenAiInvestigationCoordinatorRunnerInput) -> Self {
        Self {
            api_key: input.api_key,
            provider_settings: create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
                coordinator_model: input.coordinator_model,
            }),
            datadog_site: input.datadog_site,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl InvestigationCoordinatorRunnerPort for OpenAiInvestigationCoordinatorRunner {
    async fn run(
        &self,
        input: RunCoordinatorInput,
    ) -> Result<CoordinatorRunReport, AgentRunFailedError> {
        run_llm_coordinator(RunLlmCoordinatorInput {
            client: openai::Client::from_val(self.api_key.clone().into()),
            settings: self.provider_settings.clone(),
            datadog_site: self.datadog_site.clone(),
            github_scope_org: self.github_scope_org.clone(),
            language: self.language.clone(),
            run: input,
        })
        .await
    }
}
