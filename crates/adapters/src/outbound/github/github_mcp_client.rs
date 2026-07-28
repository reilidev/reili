use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use octocrab::auth::create_jwt;
use octocrab::models::{AppId, InstallationToken};
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use rmcp::model::Tool;
use tokio::sync::Mutex;

use crate::outbound::mcp_streamable_http::{McpHttpClientAuth, StreamableHttpMcpClient};

const GITHUB_MCP_SOURCE_LABEL: &str = "GitHub";
const GITHUB_MCP_CLIENT_NAME: &str = "reili";
const GITHUB_MCP_CLIENT_VERSION_FALLBACK: &str = "unknown";
const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_MCP_TOOLSETS_HEADER: &str = "x-mcp-toolsets";
const GITHUB_MCP_TOOLSETS: &str = "default,actions,dependabot";
const INSTALLATION_TOKEN_REFRESH_SKEW_MINUTES: i64 = 5;
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
    config: &GitHubMcpConfig,
) -> Result<(GitHubMcpHttpClient, Vec<Tool>), PortError> {
    let client = StreamableHttpMcpClient::new(
        GITHUB_MCP_SOURCE_LABEL,
        config.url.clone(),
        GitHubAppInstallationAuth::new(config)?,
    );
    let tools = client.list_tools().await?;

    Ok((client, tools))
}

/// Caches the built `reqwest::Client`, rebuilding it only when the installation token refreshes.
#[derive(Clone)]
pub(crate) struct GitHubAppInstallationAuth {
    app_id: AppId,
    installation_id: u32,
    key: Arc<jsonwebtoken::EncodingKey>,
    api_client: reqwest::Client,
    cached_auth: Arc<Mutex<Option<CachedInstallationAuth>>>,
}

#[derive(Clone)]
struct CachedInstallationAuth {
    refresh_at: DateTime<Utc>,
    http_client: reqwest::Client,
}

impl CachedInstallationAuth {
    fn new(token: &SecretString, expires_at: DateTime<Utc>) -> Result<Self, PortError> {
        let auth_header = build_bearer_auth_header(token.expose())?;
        Ok(Self {
            refresh_at: expires_at - Duration::minutes(INSTALLATION_TOKEN_REFRESH_SKEW_MINUTES),
            http_client: build_github_mcp_http_client(auth_header)?,
        })
    }

    fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        now < self.refresh_at
    }
}

impl fmt::Debug for GitHubAppInstallationAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubAppInstallationAuth")
            .field("app_id", &self.app_id)
            .field("installation_id", &self.installation_id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl GitHubAppInstallationAuth {
    fn new(config: &GitHubMcpConfig) -> Result<Self, PortError> {
        let app_id = parse_github_app_id(&config.app_id)?;
        let key = Arc::new(
            jsonwebtoken::EncodingKey::from_rsa_pem(config.private_key.as_bytes()).map_err(
                |error| {
                    PortError::invalid_input(format!(
                        "Failed to parse GitHub App private key: {error}"
                    ))
                },
            )?,
        );
        let api_client = reqwest::Client::builder()
            .user_agent(build_user_agent())
            .build()
            .map_err(|error| {
                PortError::new(format!(
                    "Failed to build GitHub App token HTTP client: {error}"
                ))
            })?;

        Ok(Self {
            app_id,
            installation_id: config.installation_id,
            key,
            api_client,
            cached_auth: Arc::new(Mutex::new(None)),
        })
    }

    async fn request_installation_token(&self) -> Result<(SecretString, DateTime<Utc>), PortError> {
        let jwt = create_jwt(self.app_id, self.key.as_ref()).map_err(|error| {
            PortError::invalid_input(format!("Failed to sign GitHub App JWT: {error}"))
        })?;
        let auth_header = build_bearer_auth_header(&jwt)?;
        let response = self
            .api_client
            .post(build_installation_token_url(self.installation_id))
            .header(AUTHORIZATION, auth_header)
            .header(ACCEPT, "application/vnd.github+json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|error| {
                PortError::connection_failed(format!(
                    "GitHub App installation token request failed: {error}"
                ))
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to read GitHub App installation token response body: {error}"
            ))
        })?;

        if !status.is_success() {
            return Err(PortError::http_status(
                status.as_u16(),
                format!(
                    "GitHub App installation token request failed: status={} body={}",
                    status.as_u16(),
                    truncate_for_error(String::from_utf8_lossy(&bytes).as_ref())
                ),
            ));
        }

        let token: InstallationToken = serde_json::from_slice(&bytes).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse GitHub App installation token response JSON: {error}"
            ))
        })?;
        let expires_at = token.expires_at.as_deref().ok_or_else(|| {
            PortError::invalid_response(
                "GitHub App installation token response did not include expires_at",
            )
        })?;
        let expires_at = DateTime::parse_from_rfc3339(expires_at)
            .map_err(|error| {
                PortError::invalid_response(format!(
                    "GitHub App installation token response had invalid expires_at: {error}"
                ))
            })?
            .with_timezone(&Utc);

        Ok((token.token.into(), expires_at))
    }
}

#[async_trait]
impl McpHttpClientAuth for GitHubAppInstallationAuth {
    async fn http_client(&self) -> Result<reqwest::Client, PortError> {
        let now = Utc::now();
        let mut cached_auth = self.cached_auth.lock().await;
        if let Some(cached_auth_value) = cached_auth.as_ref()
            && cached_auth_value.is_fresh_at(now)
        {
            return Ok(cached_auth_value.http_client.clone());
        }

        let (token, expires_at) = self.request_installation_token().await?;
        let fresh_auth = CachedInstallationAuth::new(&token, expires_at)?;
        let http_client = fresh_auth.http_client.clone();
        *cached_auth = Some(fresh_auth);

        Ok(http_client)
    }
}

fn build_user_agent() -> String {
    format!(
        "{}/{}",
        GITHUB_MCP_CLIENT_NAME, GITHUB_MCP_CLIENT_VERSION_FALLBACK
    )
}

fn build_installation_token_url(installation_id: u32) -> String {
    format!("{GITHUB_API_BASE_URL}/app/installations/{installation_id}/access_tokens")
}

fn parse_github_app_id(app_id: &str) -> Result<AppId, PortError> {
    app_id.parse::<u64>().map(AppId).map_err(|error| {
        PortError::invalid_input(format!("Invalid GitHub App ID `{app_id}`: {error}"))
    })
}

fn truncate_for_error(value: &str) -> String {
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

fn build_bearer_auth_header(token: &str) -> Result<HeaderValue, PortError> {
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
    use chrono::{Duration, Utc};
    use reili_core::secret::SecretString;
    use reqwest::header::HeaderValue;

    use super::{
        CachedInstallationAuth, GITHUB_MCP_TOOLSETS, GitHubAppInstallationAuth, GitHubMcpConfig,
        build_bearer_auth_header, build_github_mcp_headers, parse_github_app_id,
    };

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

    #[test]
    fn cached_installation_auth_refreshes_early() {
        let fresh = CachedInstallationAuth::new(
            &SecretString::from("token"),
            Utc::now() + Duration::minutes(10),
        )
        .expect("build fresh cached auth");
        let stale = CachedInstallationAuth::new(
            &SecretString::from("token"),
            Utc::now() + Duration::minutes(4),
        )
        .expect("build stale cached auth");

        assert!(fresh.is_fresh_at(Utc::now()));
        assert!(!stale.is_fresh_at(Utc::now()));
    }
}
