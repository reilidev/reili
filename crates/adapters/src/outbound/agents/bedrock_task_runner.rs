use async_trait::async_trait;
use aws_config::BehaviorVersion;
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig_bedrock::client::Client;

use super::datadog_mcp_tools::DatadogMcpToolConfig;
use super::llm_provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};
use super::llm_task_runner::{RunLlmTaskInput, run_llm_task};
use super::task_agent::TaskAgentConnectors;
use crate::outbound::github::GitHubMcpConfig;

pub struct BedrockTaskRunnerInput {
    pub model_id: String,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_mcp: GitHubMcpConfig,
    pub github_scope_org: String,
    pub connectors: TaskAgentConnectors,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct BedrockTaskRunner {
    provider_settings: LlmProviderSettings,
    aws_profile: Option<String>,
    aws_region: Option<String>,
    datadog_mcp: DatadogMcpToolConfig,
    github_mcp: GitHubMcpConfig,
    github_scope_org: String,
    connectors: TaskAgentConnectors,
    language: String,
    additional_system_prompt: Option<String>,
}

impl BedrockTaskRunner {
    pub fn new(input: BedrockTaskRunnerInput) -> Self {
        Self {
            provider_settings: create_bedrock_provider_settings(
                CreateBedrockProviderSettingsInput {
                    model_id: input.model_id,
                },
            ),
            aws_profile: input.aws_profile,
            aws_region: input.aws_region,
            datadog_mcp: input.datadog_mcp,
            github_mcp: input.github_mcp,
            github_scope_org: input.github_scope_org,
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for BedrockTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        let client =
            create_bedrock_client(self.aws_profile.as_deref(), self.aws_region.as_deref()).await;

        run_llm_task(RunLlmTaskInput {
            client,
            settings: self.provider_settings.clone(),
            datadog_mcp: self.datadog_mcp.clone(),
            github_mcp: self.github_mcp.clone(),
            github_scope_org: self.github_scope_org.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}

async fn create_bedrock_client(aws_profile: Option<&str>, aws_region: Option<&str>) -> Client {
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(profile) = aws_profile {
        loader = loader.profile_name(profile);
    }
    if let Some(region) = aws_region {
        loader = loader.region(aws_config::Region::new(region.to_string()));
    }
    let sdk_config = loader.load().await;

    Client::from(aws_sdk_bedrockruntime::Client::new(&sdk_config))
}

#[cfg(test)]
mod tests {
    use reili_core::secret::SecretString;

    use super::BedrockTaskRunnerInput;
    use crate::outbound::agents::DatadogMcpToolConfig;
    use crate::outbound::agents::TaskAgentConnectors;
    use crate::outbound::github::GitHubMcpConfig;

    #[test]
    fn input_supports_explicit_aws_profile_and_region() {
        let input = BedrockTaskRunnerInput {
            model_id: "anthropic.claude".to_string(),
            aws_profile: Some("prod-sso".to_string()),
            aws_region: Some("ap-northeast-1".to_string()),
            datadog_mcp: DatadogMcpToolConfig {
                api_key: SecretString::from("api"),
                app_key: SecretString::from("app"),
                site: "datadoghq.com".to_string(),
            },
            github_mcp: GitHubMcpConfig {
                url: "https://api.githubcopilot.com/mcp/".to_string(),
                app_id: "12345".to_string(),
                private_key: SecretString::from("private-key"),
                installation_id: 99,
            },
            github_scope_org: "example-org".to_string(),
            connectors: TaskAgentConnectors { esa: None },
            language: "English".to_string(),
            additional_system_prompt: Some("Prefer runbook links.".to_string()),
        };

        assert_eq!(input.aws_profile.as_deref(), Some("prod-sso"));
        assert_eq!(input.aws_region.as_deref(), Some("ap-northeast-1"));
        assert_eq!(
            input.additional_system_prompt.as_deref(),
            Some("Prefer runbook links.")
        );
    }
}
