use async_trait::async_trait;
use aws_config::BehaviorVersion;
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig_bedrock::client::Client;

use super::super::provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BedrockAwsConfig {
    pub profile: Option<String>,
    pub region: Option<String>,
}

pub struct BedrockTaskRunnerInput {
    pub model_id: String,
    pub sub_agent_model_id: String,
    pub aws: BedrockAwsConfig,
    pub sub_agent_aws: BedrockAwsConfig,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct BedrockTaskRunner {
    provider_settings: LlmProviderSettings,
    aws: BedrockAwsConfig,
    sub_agent_aws: BedrockAwsConfig,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl BedrockTaskRunner {
    pub fn new(input: BedrockTaskRunnerInput) -> Self {
        Self {
            provider_settings: create_bedrock_provider_settings(
                CreateBedrockProviderSettingsInput {
                    model_id: input.model_id,
                    sub_agent_model_id: input.sub_agent_model_id,
                },
            ),
            aws: input.aws,
            sub_agent_aws: input.sub_agent_aws,
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }
}

#[async_trait]
impl TaskRunnerPort for BedrockTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        let client = create_bedrock_client(&self.aws).await;

        // Reuse the lead's client when the AWS settings match, since building a fresh SDK
        // client per task isn't free.
        let sub_agent_client = if self.sub_agent_aws == self.aws {
            client.clone()
        } else {
            create_bedrock_client(&self.sub_agent_aws).await
        };

        run_task(RunLlmTaskRunnerInput {
            client,
            sub_agent_client,
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}

pub(crate) async fn create_bedrock_client(aws: &BedrockAwsConfig) -> Client {
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(profile) = aws.profile.as_deref() {
        loader = loader.profile_name(profile);
    }
    if let Some(region) = aws.region.as_deref() {
        loader = loader.region(aws_config::Region::new(region.to_string()));
    }
    let sdk_config = loader.load().await;

    Client::from(aws_sdk_bedrockruntime::Client::new(&sdk_config))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::secret::SecretString;

    use super::{BedrockAwsConfig, BedrockTaskRunnerInput};
    use crate::outbound::agents::connector::ConnectorSet;
    use crate::outbound::agents::{DatadogConnector, DatadogMcpToolConfig, GitHubConnector};
    use crate::outbound::github::GitHubMcpConfig;

    #[test]
    fn input_supports_explicit_aws_profile_and_region() {
        let connectors = ConnectorSet::new(vec![
            Arc::new(DatadogConnector::new(DatadogMcpToolConfig {
                api_key: SecretString::from("api"),
                app_key: SecretString::from("app"),
                site: "datadoghq.com".to_string(),
            })),
            Arc::new(GitHubConnector::new(
                GitHubMcpConfig {
                    url: "https://api.githubcopilot.com/mcp/".to_string(),
                    app_id: "12345".to_string(),
                    private_key: SecretString::from("private-key"),
                    installation_id: 99,
                },
                "example-org".to_string(),
            )),
        ]);
        let input = BedrockTaskRunnerInput {
            model_id: "anthropic.claude".to_string(),
            sub_agent_model_id: "moonshotai.kimi-k2.5".to_string(),
            aws: BedrockAwsConfig {
                profile: Some("prod-sso".to_string()),
                region: Some("ap-northeast-1".to_string()),
            },
            sub_agent_aws: BedrockAwsConfig {
                profile: Some("prod-sso".to_string()),
                region: Some("us-east-1".to_string()),
            },
            connectors,
            language: "English".to_string(),
            additional_system_prompt: Some("Prefer runbook links.".to_string()),
        };

        assert_eq!(input.aws.profile.as_deref(), Some("prod-sso"));
        assert_eq!(input.aws.region.as_deref(), Some("ap-northeast-1"));
        assert_eq!(input.sub_agent_aws.profile.as_deref(), Some("prod-sso"));
        assert_eq!(input.sub_agent_aws.region.as_deref(), Some("us-east-1"));
        assert_eq!(input.connectors.len(), 2);
        assert_eq!(
            input.additional_system_prompt.as_deref(),
            Some("Prefer runbook links.")
        );
    }
}
