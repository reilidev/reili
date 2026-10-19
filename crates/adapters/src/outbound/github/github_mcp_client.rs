use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use octocrab::models::AppId;
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use rmcp::model::Tool;
use tokio::sync::Mutex;

use super::app_token::GitHubAppTokenProvider;
use crate::outbound::mcp_streamable_http::{McpHttpClientAuth, StreamableHttpMcpClient};

const GITHUB_MCP_SOURCE_LABEL: &str = "GitHub";
const GITHUB_MCP_CLIENT_NAME: &str = "reili";
const GITHUB_MCP_CLIENT_VERSION_FALLBACK: &str = "unknown";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_MCP_TOOLSETS_HEADER: &str = "x-mcp-toolsets";
const GITHUB_MCP_TOOLSETS: &str = "default,actions,dependabot";
const MAX_ERROR_BODY_CHARS: usize = 500;

#[derive(Clone, PartialEq, Eq)]
pub struct GitHubMcpConfig {
    pub url: String,
    pub app_id: String,
    pub private_key: SecretString,
    pub installation_id: u32,
}

impl fmt::Debug for GitHubMcpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubMcpConfig")
            .field("url", &self.url)
            .field("app_id", &self.app_id)
            .field("private_key", &"[REDACTED]")
            .field("installation_id", &self.installation_id)
            .finish()
    }
}

pub(crate) type GitHubMcpHttpClient = StreamableHttpMcpClient<GitHubAppInstallationAuth>;

pub(crate) async fn connect(
    url: &str,
    auth: GitHubAppInstallationAuth,
) -> Result<(GitHubMcpHttpClient, Vec<Tool>), PortError> {
    let client = StreamableHttpMcpClient::new(GITHUB_MCP_SOURCE_LABEL, url.to_string(), auth);
    let tools = client.list_tools().await?;

    Ok((client, tools))
}

/// Rebuilds the `reqwest::Client` only when the shared provider hands out a new token.
#[derive(Clone)]
pub(crate) struct GitHubAppInstallationAuth {
    tokens: Arc<GitHubAppTokenProvider>,
    cached_client: Arc<Mutex<Option<(SecretString, reqwest::Client)>>>,
}

impl fmt::Debug for GitHubAppInstallationAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubAppInstallationAuth")
            .finish_non_exhaustive()
    }
}

impl GitHubAppInstallationAuth {
    pub(crate) fn new(config: &GitHubMcpConfig) -> Result<Self, PortError> {
        Ok(Self::with_token_provider(GitHubAppTokenProvider::new(
            &config.app_id,
            &config.private_key,
            config.installation_id,
            GITHUB_API_BASE_URL,
            &build_user_agent(),
        )?))
    }

    fn with_token_provider(tokens: GitHubAppTokenProvider) -> Self {
        Self {
            tokens: Arc::new(tokens),
            cached_client: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl McpHttpClientAuth for GitHubAppInstallationAuth {
    async fn http_client(&self) -> Result<reqwest::Client, PortError> {
        let token = self.tokens.token().await?;
        let mut cached_client = self.cached_client.lock().await;
        if let Some((cached_token, client)) = cached_client.as_ref()
            && cached_token.expose() == token.expose()
        {
            return Ok(client.clone());
        }

        let client = build_github_mcp_http_client(build_bearer_auth_header(token.expose())?)?;
        *cached_client = Some((token, client.clone()));

        Ok(client)
    }
}

fn build_user_agent() -> String {
    format!(
        "{}/{}",
        GITHUB_MCP_CLIENT_NAME, GITHUB_MCP_CLIENT_VERSION_FALLBACK
    )
}

pub(crate) fn parse_github_app_id(app_id: &str) -> Result<AppId, PortError> {
    app_id.parse::<u64>().map(AppId).map_err(|error| {
        PortError::invalid_input(format!("Invalid GitHub App ID `{app_id}`: {error}"))
    })
}

pub(crate) fn truncate_for_error(value: &str) -> String {
    let mut truncated = value.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>();
    if value.chars().count() > MAX_ERROR_BODY_CHARS {
        truncated.push_str("...");
    }
    truncated
}

fn build_github_mcp_http_client(auth_header: HeaderValue) -> Result<reqwest::Client, PortError> {
    reqwest::Client::builder()
        .default_headers(build_github_mcp_headers(auth_header))
        .user_agent(build_user_agent())
        .build()
        .map_err(|error| {
            PortError::new(format!(
                "Failed to build GitHub MCP HTTP client with default headers: {error}"
            ))
        })
}

fn build_github_mcp_headers(auth_header: HeaderValue) -> HeaderMap {
    let mut default_headers = HeaderMap::new();
    default_headers.insert(AUTHORIZATION, auth_header);
    default_headers.insert(
        HeaderName::from_static(GITHUB_MCP_TOOLSETS_HEADER),
        HeaderValue::from_static(GITHUB_MCP_TOOLSETS),
    );

    default_headers
}

pub(crate) fn build_bearer_auth_header(token: &str) -> Result<HeaderValue, PortError> {
    let token = token.trim();
    let header_value = if token.starts_with("Bearer ") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    };

    HeaderValue::from_str(&header_value).map_err(|error| {
        PortError::new(format!("Invalid GitHub MCP authorization header: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use reili_core::secret::SecretString;
    use reqwest::header::HeaderValue;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        GITHUB_MCP_TOOLSETS, GitHubAppInstallationAuth, GitHubMcpConfig, build_bearer_auth_header,
        build_github_mcp_headers, parse_github_app_id,
    };
    use crate::outbound::github::app_token::{GitHubAppTokenProvider, test_app_private_key};
    use crate::outbound::mcp_streamable_http::McpHttpClientAuth;

    #[test]
    fn builds_bearer_authorization_header() {
        let header = build_bearer_auth_header("token").expect("build auth header");

        assert_eq!(header, HeaderValue::from_static("Bearer token"));
    }

    #[test]
    fn preserves_existing_bearer_authorization_header() {
        let header = build_bearer_auth_header("Bearer token").expect("build auth header");

        assert_eq!(header, HeaderValue::from_static("Bearer token"));
    }

    #[test]
    fn builds_github_mcp_headers() {
        let headers =
            build_github_mcp_headers(build_bearer_auth_header("token").expect("build auth header"));

        assert_eq!(
            headers.get(reqwest::header::AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer token"))
        );
        assert_eq!(
            headers.get("x-mcp-toolsets"),
            Some(&HeaderValue::from_static(GITHUB_MCP_TOOLSETS))
        );
    }

    #[test]
    fn parses_github_app_id() {
        assert_eq!(
            parse_github_app_id("123").expect("parse app id"),
            octocrab::models::AppId(123)
        );
    }

    #[test]
    fn rejects_invalid_private_key() {
        let error = GitHubAppInstallationAuth::new(&GitHubMcpConfig {
            url: "https://api.githubcopilot.com/mcp/".to_string(),
            app_id: "123".to_string(),
            private_key: SecretString::from("invalid"),
            installation_id: 456,
        })
        .expect_err("invalid private key should fail");

        assert!(error.is_invalid_input());
    }

    #[tokio::test]
    async fn clones_share_one_installation_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "token": "ghs_test",
                "permissions": {},
                "expires_at": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })))
            .expect(1)
            .mount(&server)
            .await;
        let auth = GitHubAppInstallationAuth::with_token_provider(
            GitHubAppTokenProvider::new(
                "123",
                &test_app_private_key(),
                7,
                &server.uri(),
                "reili-test",
            )
            .expect("provider"),
        );

        auth.http_client().await.expect("first run");
        auth.clone().http_client().await.expect("second run");

        server.verify().await;
    }
}
