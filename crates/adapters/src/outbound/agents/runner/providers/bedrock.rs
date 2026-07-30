use async_trait::async_trait;
use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, ConfigLoader, SdkConfig};
use aws_credential_types::provider::ProvideCredentials;
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig_bedrock::client::Client;

use super::super::provider_settings::{
    CreateBedrockProviderSettingsInput, LlmProviderSettings, create_bedrock_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

const ASSUME_ROLE_SESSION_NAME: &str = "reili";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BedrockAwsConfig {
    pub profile: Option<String>,
    pub region: Option<String>,
    pub assume_role_arn: Option<String>,
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
    client: Client,
    sub_agent_client: Client,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl BedrockTaskRunner {
    pub async fn new(input: BedrockTaskRunnerInput) -> Result<Self, PortError> {
        let client = create_bedrock_client(&input.aws).await?;

        let sub_agent_client = if input.sub_agent_aws == input.aws {
            client.clone()
        } else {
            create_bedrock_client(&input.sub_agent_aws).await?
        };

        Ok(Self {
            provider_settings: create_bedrock_provider_settings(
                CreateBedrockProviderSettingsInput {
                    model_id: input.model_id,
                    sub_agent_model_id: input.sub_agent_model_id,
                },
            ),
            client,
            sub_agent_client,
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        })
    }
}

#[async_trait]
impl TaskRunnerPort for BedrockTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        run_task(RunLlmTaskRunnerInput {
            client: self.client.clone(),
            sub_agent_client: self.sub_agent_client.clone(),
            settings: self.provider_settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
            run: input,
        })
        .await
    }
}

pub(crate) async fn create_bedrock_client(aws: &BedrockAwsConfig) -> Result<Client, PortError> {
    let sdk_config = load_sdk_config(aws).await;
    verify_credentials(aws, &sdk_config).await?;

    Ok(Client::from(aws_sdk_bedrockruntime::Client::new(
        &sdk_config,
    )))
}

async fn load_sdk_config(aws: &BedrockAwsConfig) -> SdkConfig {
    let base_config = config_loader(aws).load().await;

    let Some(assume_role_arn) = aws.assume_role_arn.as_deref() else {
        return base_config;
    };

    let assume_role_provider = AssumeRoleProvider::builder(assume_role_arn)
        .session_name(ASSUME_ROLE_SESSION_NAME)
        .configure(&base_config)
        .build()
        .await;

    config_loader(aws)
        .credentials_provider(assume_role_provider)
        .load()
        .await
}

fn config_loader(aws: &BedrockAwsConfig) -> ConfigLoader {
    let mut loader = aws_config::defaults(BehaviorVersion::latest());
    if let Some(profile) = aws.profile.as_deref() {
        loader = loader.profile_name(profile);
    }
    if let Some(region) = aws.region.as_deref() {
        loader = loader.region(aws_config::Region::new(region.to_string()));
    }
    loader
}

async fn verify_credentials(
    aws: &BedrockAwsConfig,
    sdk_config: &SdkConfig,
) -> Result<(), PortError> {
    let provider = sdk_config
        .credentials_provider()
        .ok_or_else(|| PortError::new(credentials_error_message(aws, "no credentials provider")))?;

    provider
        .provide_credentials()
        .await
        .map_err(|error| PortError::new(credentials_error_message(aws, &error.to_string())))?;

    Ok(())
}

fn credentials_error_message(aws: &BedrockAwsConfig, cause: &str) -> String {
    let target = match (aws.assume_role_arn.as_deref(), aws.profile.as_deref()) {
        (Some(assume_role_arn), Some(profile)) => {
            format!("role `{assume_role_arn}` from profile `{profile}`")
        }
        (Some(assume_role_arn), None) => format!("role `{assume_role_arn}`"),
        (None, Some(profile)) => format!("profile `{profile}`"),
        (None, None) => "the default AWS credential chain".to_string(),
    };

    format!("failed to resolve AWS credentials for Bedrock using {target}: {cause}")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::secret::SecretString;

    use super::{BedrockAwsConfig, BedrockTaskRunnerInput, credentials_error_message};
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
                assume_role_arn: Some("arn:aws:iam::111111111111:role/ReiliLead".to_string()),
            },
            sub_agent_aws: BedrockAwsConfig {
                profile: Some("prod-sso".to_string()),
                region: Some("us-east-1".to_string()),
                assume_role_arn: Some("arn:aws:iam::222222222222:role/ReiliSubAgent".to_string()),
            },
            connectors,
            language: "English".to_string(),
            additional_system_prompt: Some("Prefer runbook links.".to_string()),
        };

        assert_eq!(input.aws.profile.as_deref(), Some("prod-sso"));
        assert_eq!(input.aws.region.as_deref(), Some("ap-northeast-1"));
        assert_eq!(
            input.aws.assume_role_arn.as_deref(),
            Some("arn:aws:iam::111111111111:role/ReiliLead")
        );
        assert_eq!(input.sub_agent_aws.profile.as_deref(), Some("prod-sso"));
        assert_eq!(input.sub_agent_aws.region.as_deref(), Some("us-east-1"));
        assert_eq!(
            input.sub_agent_aws.assume_role_arn.as_deref(),
            Some("arn:aws:iam::222222222222:role/ReiliSubAgent")
        );
        assert_eq!(input.connectors.len(), 2);
        assert_eq!(
            input.additional_system_prompt.as_deref(),
            Some("Prefer runbook links.")
        );
    }

    #[test]
    fn aws_config_without_assume_role_arn_stays_equal_to_the_profile_only_default() {
        let aws = BedrockAwsConfig {
            profile: Some("prod-sso".to_string()),
            region: Some("us-east-1".to_string()),
            assume_role_arn: None,
        };
        let with_role = BedrockAwsConfig {
            assume_role_arn: Some("arn:aws:iam::111111111111:role/ReiliLead".to_string()),
            ..aws.clone()
        };

        assert_eq!(aws, aws.clone());
        assert_ne!(aws, with_role);
    }

    #[test]
    fn credentials_error_message_names_the_role_and_profile_it_tried() {
        let message = credentials_error_message(
            &BedrockAwsConfig {
                profile: Some("prod-sso".to_string()),
                region: Some("us-east-1".to_string()),
                assume_role_arn: Some("arn:aws:iam::111111111111:role/ReiliLead".to_string()),
            },
            "AccessDenied",
        );

        assert!(
            message.contains("role `arn:aws:iam::111111111111:role/ReiliLead`"),
            "{message}"
        );
        assert!(message.contains("profile `prod-sso`"), "{message}");
        assert!(message.contains("AccessDenied"), "{message}");
    }

    #[test]
    fn credentials_error_message_falls_back_to_the_default_chain() {
        let message = credentials_error_message(&BedrockAwsConfig::default(), "no credentials");

        assert!(
            message.contains("the default AWS credential chain"),
            "{message}"
        );
    }
}
