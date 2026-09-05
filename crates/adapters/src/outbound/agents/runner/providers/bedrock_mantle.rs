use std::future::Future;
use std::time::SystemTime;

use async_trait::async_trait;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use bytes::Bytes;
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::secret::SecretString;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig::http_client::{
    Error as HttpClientError, HeaderMap, HeaderValue, HttpClientExt, LazyBody, Method,
    MultipartForm, Request, Response, Result as HttpClientResult, StreamingResponse, Uri,
};
use rig::providers::{anthropic, openai};
use rig::wasm_compat::WasmCompatSend;

use super::super::provider_settings::{
    CreateBedrockMantleProviderSettingsInput, LlmProviderSettings,
    create_bedrock_mantle_provider_settings,
};
use super::super::task_runner::{RunLlmTaskRunnerInput, run_task};
use super::bedrock::{BedrockAwsConfig, load_sdk_config};
use crate::outbound::agents::connector::ConnectorSet;

const BEDROCK_MANTLE_SIGNING_SERVICE: &str = "bedrock-mantle";
/// Bedrock Mantle authenticates via a `Bearer` `Authorization` header (API key) or SigV4 (IAM
/// role); both auth modes are set explicitly through [`bearer_authorization_header`] rather than
/// each provider client's own `.api_key(...)`, so this placeholder is only ever there to satisfy
/// `ClientBuilder::build`'s typestate — it never reaches AWS.
const BEDROCK_MANTLE_UNUSED_API_KEY_PLACEHOLDER: &str = "unused-bedrock-mantle-client-api-key";
/// Stand-in `Authorization` value for IAM-role auth, always overwritten by
/// [`SigV4Signer::sign_headers`] before the request leaves the process.
const BEDROCK_MANTLE_IAM_ROLE_PLACEHOLDER_BEARER_VALUE: &str = "unused-bedrock-mantle-sigv4-auth";

/// Credentials for a Bedrock Mantle backend. Bedrock Mantle authenticates requests either with a
/// bearer API key or by SigV4-signing the request with AWS credentials; see
/// <https://docs.aws.amazon.com/bedrock/latest/userguide/bedrock-mantle.html>.
#[derive(Clone)]
pub enum BedrockMantleAuth {
    ApiKey(SecretString),
    IamRole(BedrockMantleIamRole),
}

/// AWS identity used to SigV4-sign Bedrock Mantle requests. Unlike [`BedrockAwsConfig`] this
/// carries no region of its own: a Mantle backend's region is fixed by its endpoint, so the
/// signer always signs for that same region.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BedrockMantleIamRole {
    pub profile: Option<String>,
    pub assume_role_arn: Option<String>,
}

/// Which Bedrock Mantle API family a model belongs to, inferred from its model ID prefix.
/// Bedrock Mantle hosts different model providers on different paths and wire formats:
/// OpenAI and xAI models speak the OpenAI Responses API under `/openai/v1`, while Anthropic
/// models speak the native Anthropic Messages API under `/anthropic/v1`. Confirmed against the
/// AWS model cards for
/// [GPT-5.4](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-openai-gpt-54.html),
/// [Grok 4.6](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-xai-grok-4-6.html),
/// and [Claude Mythos 5](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-mythos-5.html).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BedrockMantleModelFamily {
    OpenAiCompatible,
    Anthropic,
}

impl BedrockMantleModelFamily {
    pub(crate) fn from_model_id(model_id: &str) -> Result<Self, PortError> {
        if model_id.starts_with("openai.") || model_id.starts_with("xai.") {
            Ok(Self::OpenAiCompatible)
        } else if model_id.starts_with("anthropic.") {
            Ok(Self::Anthropic)
        } else {
            Err(PortError::new(format!(
                "unrecognized Bedrock Mantle model `{model_id}`: expected an `openai.`, `xai.`, or `anthropic.` prefixed model ID"
            )))
        }
    }
}

pub struct BedrockMantleTaskRunnerInput {
    pub model_id: String,
    pub sub_agent_model_id: String,
    pub region: String,
    pub auth: BedrockMantleAuth,
    pub sub_agent_region: String,
    pub sub_agent_auth: BedrockMantleAuth,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

pub struct BedrockMantleTaskRunner {
    provider_settings: LlmProviderSettings,
    client: BedrockMantleClient,
    sub_agent_client: BedrockMantleClient,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl BedrockMantleTaskRunner {
    pub async fn new(input: BedrockMantleTaskRunnerInput) -> Result<Self, PortError> {
        let family = BedrockMantleModelFamily::from_model_id(&input.model_id)?;
        let sub_agent_family = BedrockMantleModelFamily::from_model_id(&input.sub_agent_model_id)?;
        if family != sub_agent_family {
            return Err(PortError::new(format!(
                "lead model `{}` and sub-agent model `{}` belong to different Bedrock Mantle API families; both must be openai/xai-family or both anthropic-family",
                input.model_id, input.sub_agent_model_id
            )));
        }

        let client = create_bedrock_mantle_client(family, &input.region, &input.auth).await?;
        let sub_agent_client =
            create_bedrock_mantle_client(family, &input.sub_agent_region, &input.sub_agent_auth)
                .await?;

        Ok(Self {
            provider_settings: create_bedrock_mantle_provider_settings(
                CreateBedrockMantleProviderSettingsInput {
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
impl TaskRunnerPort for BedrockMantleTaskRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        match (&self.client, &self.sub_agent_client) {
            (
                BedrockMantleClient::OpenAi(client),
                BedrockMantleClient::OpenAi(sub_agent_client),
            ) => {
                run_task(RunLlmTaskRunnerInput {
                    client: client.clone(),
                    sub_agent_client: sub_agent_client.clone(),
                    settings: self.provider_settings.clone(),
                    connectors: self.connectors.clone(),
                    language: self.language.clone(),
                    additional_system_prompt: self.additional_system_prompt.clone(),
                    run: input,
                })
                .await
            }
            (
                BedrockMantleClient::Anthropic(client),
                BedrockMantleClient::Anthropic(sub_agent_client),
            ) => {
                run_task(RunLlmTaskRunnerInput {
                    client: client.clone(),
                    sub_agent_client: sub_agent_client.clone(),
                    settings: self.provider_settings.clone(),
                    connectors: self.connectors.clone(),
                    language: self.language.clone(),
                    additional_system_prompt: self.additional_system_prompt.clone(),
                    run: input,
                })
                .await
            }
            _ => unreachable!(
                "lead/sub-agent Bedrock Mantle model family mismatch already rejected in BedrockMantleTaskRunner::new"
            ),
        }
    }
}

/// The rig completion client for a Bedrock Mantle backend. Which variant is built is decided by
/// [`BedrockMantleModelFamily`], since OpenAI/xAI and Anthropic models speak different wire
/// protocols on Bedrock Mantle and therefore need different rig provider clients.
#[derive(Clone)]
pub(crate) enum BedrockMantleClient {
    OpenAi(openai::Client<BedrockMantleHttpClient>),
    Anthropic(anthropic::Client<BedrockMantleHttpClient>),
}

pub(crate) async fn create_bedrock_mantle_client(
    family: BedrockMantleModelFamily,
    region: &str,
    auth: &BedrockMantleAuth,
) -> Result<BedrockMantleClient, PortError> {
    let bearer_value = match auth {
        BedrockMantleAuth::ApiKey(secret) => secret.expose().to_string(),
        BedrockMantleAuth::IamRole(_) => {
            BEDROCK_MANTLE_IAM_ROLE_PLACEHOLDER_BEARER_VALUE.to_string()
        }
    };
    let authorization_headers = bearer_authorization_header(&bearer_value).map_err(|error| {
        PortError::new(format!("failed to build Bedrock Mantle client: {error}"))
    })?;

    let http_client = match auth {
        BedrockMantleAuth::ApiKey(_) => BedrockMantleHttpClient::Bearer(reqwest::Client::new()),
        BedrockMantleAuth::IamRole(role) => {
            let signer = build_sigv4_signer(region, role).await?;
            BedrockMantleHttpClient::SigV4 {
                inner: reqwest::Client::new(),
                signer,
            }
        }
    };

    let base_url = bedrock_mantle_base_url(family, region);
    let build_error = |error: rig::http_client::Error| {
        PortError::new(format!("failed to build Bedrock Mantle client: {error}"))
    };

    match family {
        BedrockMantleModelFamily::OpenAiCompatible => openai::Client::builder()
            .base_url(base_url)
            .http_headers(authorization_headers)
            .api_key(BEDROCK_MANTLE_UNUSED_API_KEY_PLACEHOLDER)
            .http_client(http_client)
            .build()
            .map(BedrockMantleClient::OpenAi)
            .map_err(build_error),
        BedrockMantleModelFamily::Anthropic => anthropic::Client::builder()
            .base_url(base_url)
            .http_headers(authorization_headers)
            .api_key(BEDROCK_MANTLE_UNUSED_API_KEY_PLACEHOLDER)
            .http_client(http_client)
            .build()
            .map(BedrockMantleClient::Anthropic)
            .map_err(build_error),
    }
}

fn bearer_authorization_header(value: &str) -> HttpClientResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {value}")).map_err(HttpClientError::from)?,
    );
    Ok(headers)
}

/// OpenAI/xAI models on Bedrock Mantle are served under `/openai/v1`; Anthropic models under
/// `/anthropic/v1`. See [`BedrockMantleModelFamily`] for the sources.
fn bedrock_mantle_base_url(family: BedrockMantleModelFamily, region: &str) -> String {
    let path = match family {
        BedrockMantleModelFamily::OpenAiCompatible => "openai",
        BedrockMantleModelFamily::Anthropic => "anthropic",
    };
    format!("https://bedrock-mantle.{region}.api.aws/{path}/v1")
}

async fn build_sigv4_signer(
    region: &str,
    role: &BedrockMantleIamRole,
) -> Result<SigV4Signer, PortError> {
    let aws = BedrockAwsConfig {
        profile: role.profile.clone(),
        region: Some(region.to_string()),
        assume_role_arn: role.assume_role_arn.clone(),
    };
    let sdk_config = load_sdk_config(&aws).await;
    let credentials_provider = sdk_config.credentials_provider().ok_or_else(|| {
        PortError::new(format!(
            "failed to resolve AWS credentials for Bedrock Mantle in region `{region}`: no credentials provider"
        ))
    })?;
    verify_credentials(region, &credentials_provider).await?;

    Ok(SigV4Signer {
        credentials_provider,
        region: region.to_string(),
    })
}

async fn verify_credentials(
    region: &str,
    credentials_provider: &SharedCredentialsProvider,
) -> Result<(), PortError> {
    credentials_provider
        .provide_credentials()
        .await
        .map_err(|error| {
            PortError::new(format!(
                "failed to resolve AWS credentials for Bedrock Mantle in region `{region}`: {error}"
            ))
        })?;

    Ok(())
}

/// SigV4-signs outgoing Bedrock Mantle requests.
#[derive(Debug, Clone)]
pub(crate) struct SigV4Signer {
    credentials_provider: SharedCredentialsProvider,
    region: String,
}

impl SigV4Signer {
    async fn sign_headers(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: SignableBody<'_>,
    ) -> HttpClientResult<HeaderMap> {
        let credentials = self
            .credentials_provider
            .provide_credentials()
            .await
            .map_err(signing_instance_error)?;
        let identity = credentials.into();
        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name(BEDROCK_MANTLE_SIGNING_SERVICE)
            .time(SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .map_err(signing_instance_error)?
            .into();

        let header_pairs = headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value)));
        let signable_request =
            SignableRequest::new(method.as_str(), uri.to_string(), header_pairs, body)
                .map_err(signing_instance_error)?;

        let (instructions, _signature) = sign(signable_request, &signing_params)
            .map_err(signing_instance_error)?
            .into_parts();

        let mut scratch_request = http::Request::builder()
            .method(method.clone())
            .uri(uri.clone())
            .body(())
            .map_err(HttpClientError::Protocol)?;
        *scratch_request.headers_mut() = headers.clone();
        instructions.apply_to_request_http1x(&mut scratch_request);

        Ok(scratch_request.into_parts().0.headers)
    }
}

fn signing_instance_error<E>(error: E) -> HttpClientError
where
    E: std::error::Error + Send + Sync + 'static,
{
    HttpClientError::Instance(Box::new(error))
}

#[derive(Debug, Clone)]
pub(crate) enum BedrockMantleHttpClient {
    Bearer(reqwest::Client),
    SigV4 {
        inner: reqwest::Client,
        signer: SigV4Signer,
    },
}

impl Default for BedrockMantleHttpClient {
    fn default() -> Self {
        Self::Bearer(reqwest::Client::default())
    }
}

impl HttpClientExt for BedrockMantleHttpClient {
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = HttpClientResult<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes>,
        T: WasmCompatSend,
        U: From<Bytes>,
        U: WasmCompatSend + 'static,
    {
        let client = self.clone();
        let (parts, body) = req.into_parts();
        let body: Bytes = body.into();
        async move {
            match client {
                Self::Bearer(inner) => inner.send(Request::from_parts(parts, body)).await,
                Self::SigV4 { inner, signer } => {
                    let mut parts = parts;
                    parts.headers = signer
                        .sign_headers(
                            &parts.method,
                            &parts.uri,
                            &parts.headers,
                            SignableBody::Bytes(&body),
                        )
                        .await?;
                    inner.send(Request::from_parts(parts, body)).await
                }
            }
        }
    }

    fn send_multipart<U>(
        &self,
        req: Request<MultipartForm>,
    ) -> impl Future<Output = HttpClientResult<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes>,
        U: WasmCompatSend + 'static,
    {
        let client = self.clone();
        let (parts, body) = req.into_parts();
        async move {
            match client {
                Self::Bearer(inner) => inner.send_multipart(Request::from_parts(parts, body)).await,
                Self::SigV4 { inner, signer } => {
                    let mut parts = parts;
                    // Multipart bodies stream lazily, so there's no payload to hash up front;
                    // sign as unsigned payload instead (still SigV4-authenticated via headers).
                    parts.headers = signer
                        .sign_headers(
                            &parts.method,
                            &parts.uri,
                            &parts.headers,
                            SignableBody::UnsignedPayload,
                        )
                        .await?;
                    inner.send_multipart(Request::from_parts(parts, body)).await
                }
            }
        }
    }

    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = HttpClientResult<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes>,
    {
        let client = self.clone();
        let (parts, body) = req.into_parts();
        let body: Bytes = body.into();
        async move {
            match client {
                Self::Bearer(inner) => inner.send_streaming(Request::from_parts(parts, body)).await,
                Self::SigV4 { inner, signer } => {
                    let mut parts = parts;
                    parts.headers = signer
                        .sign_headers(
                            &parts.method,
                            &parts.uri,
                            &parts.headers,
                            SignableBody::Bytes(&body),
                        )
                        .await?;
                    inner.send_streaming(Request::from_parts(parts, body)).await
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::secret::SecretString;

    use super::{
        BedrockMantleAuth, BedrockMantleIamRole, BedrockMantleModelFamily,
        BedrockMantleTaskRunnerInput, bedrock_mantle_base_url,
    };
    use crate::outbound::agents::connector::ConnectorSet;
    use crate::outbound::agents::{DatadogConnector, DatadogMcpToolConfig, GitHubConnector};
    use crate::outbound::github::GitHubMcpConfig;

    #[test]
    fn base_url_targets_the_family_specific_regional_path() {
        assert_eq!(
            bedrock_mantle_base_url(BedrockMantleModelFamily::OpenAiCompatible, "us-east-1"),
            "https://bedrock-mantle.us-east-1.api.aws/openai/v1"
        );
        assert_eq!(
            bedrock_mantle_base_url(BedrockMantleModelFamily::Anthropic, "us-east-1"),
            "https://bedrock-mantle.us-east-1.api.aws/anthropic/v1"
        );
    }

    #[test]
    fn classifies_model_family_from_id_prefix() {
        assert_eq!(
            BedrockMantleModelFamily::from_model_id("openai.gpt-5.6-sol").unwrap(),
            BedrockMantleModelFamily::OpenAiCompatible
        );
        assert_eq!(
            BedrockMantleModelFamily::from_model_id("xai.grok-4.6").unwrap(),
            BedrockMantleModelFamily::OpenAiCompatible
        );
        assert_eq!(
            BedrockMantleModelFamily::from_model_id("anthropic.claude-mythos-5").unwrap(),
            BedrockMantleModelFamily::Anthropic
        );
    }

    #[test]
    fn rejects_unrecognized_model_id_prefix() {
        let error = BedrockMantleModelFamily::from_model_id("amazon.nova-pro").unwrap_err();

        assert!(
            error.message.contains("amazon.nova-pro"),
            "{}",
            error.message
        );
    }

    #[test]
    fn input_supports_api_key_and_iam_role_auth_per_role() {
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
        let input = BedrockMantleTaskRunnerInput {
            model_id: "openai.gpt-5.6-sol".to_string(),
            sub_agent_model_id: "openai.gpt-5.6-terra".to_string(),
            region: "us-east-1".to_string(),
            auth: BedrockMantleAuth::ApiKey(SecretString::from("mantle-api-key")),
            sub_agent_region: "us-west-2".to_string(),
            sub_agent_auth: BedrockMantleAuth::IamRole(BedrockMantleIamRole {
                profile: Some("prod-sso".to_string()),
                assume_role_arn: Some("arn:aws:iam::111111111111:role/ReiliMantle".to_string()),
            }),
            connectors,
            language: "English".to_string(),
            additional_system_prompt: Some("Prefer runbook links.".to_string()),
        };

        assert_eq!(input.region, "us-east-1");
        match input.auth {
            BedrockMantleAuth::ApiKey(secret) => assert_eq!(secret.expose(), "mantle-api-key"),
            BedrockMantleAuth::IamRole(_) => panic!("expected api key auth"),
        }
        assert_eq!(input.sub_agent_region, "us-west-2");
        match input.sub_agent_auth {
            BedrockMantleAuth::IamRole(role) => {
                assert_eq!(role.profile.as_deref(), Some("prod-sso"));
                assert_eq!(
                    role.assume_role_arn.as_deref(),
                    Some("arn:aws:iam::111111111111:role/ReiliMantle")
                );
            }
            BedrockMantleAuth::ApiKey(_) => panic!("expected iam role auth"),
        }
        assert_eq!(input.connectors.len(), 2);
        assert_eq!(
            input.additional_system_prompt.as_deref(),
            Some("Prefer runbook links.")
        );
    }
}
