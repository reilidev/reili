use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, ConfigLoader, SdkConfig};
use aws_credential_types::provider::ProvideCredentials;
use reili_core::error::PortError;
use rig_bedrock::client::Client;

use super::super::llm_runner::{AgentClients, LlmBackend, LlmClients};
use super::super::provider_settings::{
    CreateBedrockProviderSettingsInput, create_bedrock_provider_settings,
};

const ASSUME_ROLE_SESSION_NAME: &str = "reili";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BedrockAwsConfig {
    pub profile: Option<String>,
    pub region: Option<String>,
    pub assume_role_arn: Option<String>,
}

pub struct BedrockBackendInput {
    pub model_id: String,
    pub sub_agent_model_id: String,
    pub aws: BedrockAwsConfig,
    pub sub_agent_aws: BedrockAwsConfig,
}

pub async fn bedrock_backend(input: BedrockBackendInput) -> Result<LlmBackend, PortError> {
    let lead = create_bedrock_client(&input.aws).await?;
    let sub_agent = if input.sub_agent_aws == input.aws {
        lead.clone()
    } else {
        create_bedrock_client(&input.sub_agent_aws).await?
    };
    Ok(LlmBackend {
        clients: LlmClients::Bedrock(AgentClients { lead, sub_agent }),
        settings: create_bedrock_provider_settings(CreateBedrockProviderSettingsInput {
            model_id: input.model_id,
            sub_agent_model_id: input.sub_agent_model_id,
        }),
    })
}

pub(crate) async fn create_bedrock_client(aws: &BedrockAwsConfig) -> Result<Client, PortError> {
    let sdk_config = load_sdk_config(aws).await;
    verify_credentials(aws, &sdk_config).await?;

    Ok(Client::from(aws_sdk_bedrockruntime::Client::new(
        &sdk_config,
    )))
}

pub(crate) async fn load_sdk_config(aws: &BedrockAwsConfig) -> SdkConfig {
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
    use super::{BedrockAwsConfig, credentials_error_message};

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
