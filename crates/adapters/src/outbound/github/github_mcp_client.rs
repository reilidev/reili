use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use octocrab::auth::create_jwt;
use octocrab::models::{AppId, InstallationToken};
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientJsonRpcMessage, ClientNotification, ClientRequest, Implementation, InitializeRequest,
    InitializedNotification, ListToolsRequest, NumberOrString, RequestId, ServerJsonRpcMessage,
    ServerResult, Tool,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use tracing::error;

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

#[derive(Clone, Debug)]
pub(crate) struct GitHubMcpHttpClient {
    uri: Arc<str>,
    client_info: ClientInfo,
    request_id: Arc<AtomicU32>,
    auth: GitHubAppInstallationAuth,
}

#[derive(Clone)]
struct GitHubAppInstallationAuth {
    app_id: AppId,
    installation_id: u32,
    key: Arc<jsonwebtoken::EncodingKey>,
    api_client: reqwest::Client,
    cached_token: Arc<Mutex<Option<CachedInstallationToken>>>,
}

#[derive(Clone, Debug)]
struct CachedInstallationToken {
    token: SecretString,
    refresh_at: DateTime<Utc>,
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

impl GitHubMcpHttpClient {
    pub(crate) fn new(config: &GitHubMcpConfig) -> Result<Self, PortError> {
        Ok(Self {
            uri: config.url.clone().into(),
            client_info: build_client_info(),
            request_id: Arc::new(AtomicU32::new(1)),
            auth: GitHubAppInstallationAuth::new(config)?,
        })
    }

    pub(crate) async fn connect(config: &GitHubMcpConfig) -> Result<(Self, Vec<Tool>), PortError> {
        let client = Self::new(config)?;
        let tools = client.list_tools().await?;

        Ok((client, tools))
    }

    pub(crate) async fn call_tool(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
    ) -> Result<CallToolResult, PortError> {
        let session_id = self.initialize_session().await?;
        let result = self
            .call_tool_with_session(name, arguments, session_id.clone())
            .await;
        self.cleanup_session(session_id).await;
        result
    }

    async fn list_tools(&self) -> Result<Vec<Tool>, PortError> {
        let session_id = self.initialize_session().await?;
        let result = self.list_tools_with_session(session_id.clone()).await;
        self.cleanup_session(session_id).await;
        result
    }

    async fn initialize_session(&self) -> Result<Option<Arc<str>>, PortError> {
        let initialize_request: ClientRequest =
            InitializeRequest::new(self.client_info.clone()).into();
        let initialize_response = self
            .authorized_mcp_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(initialize_request, self.next_request_id()),
                None,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| {
                format_streamable_http_error("initialize GitHub MCP session", error)
            })?;
        let (initialize_result, session_id) = read_server_result(initialize_response).await?;
        match initialize_result {
            ServerResult::InitializeResult(_) => {}
            other => {
                return Err(PortError::new(format!(
                    "GitHub MCP initialize returned unexpected result: {other:?}"
                )));
            }
        }

        self.send_initialized_notification(session_id.clone())
            .await?;
        Ok(session_id)
    }

    async fn send_initialized_notification(
        &self,
        session_id: Option<Arc<str>>,
    ) -> Result<(), PortError> {
        let notification: ClientNotification = InitializedNotification {
            method: Default::default(),
            extensions: Default::default(),
        }
        .into();
        self.authorized_mcp_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::notification(notification),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| {
                format_streamable_http_error("send initialized notification to GitHub MCP", error)
            })?
            .expect_accepted_or_json::<reqwest::Error>()
            .map_err(|error| {
                format_streamable_http_error(
                    "process initialized notification response from GitHub MCP",
                    error,
                )
            })?;

        Ok(())
    }

    async fn list_tools_with_session(
        &self,
        session_id: Option<Arc<str>>,
    ) -> Result<Vec<Tool>, PortError> {
        let list_tools_request: ClientRequest = ListToolsRequest {
            method: Default::default(),
            params: None,
            extensions: Default::default(),
        }
        .into();
        let response = self
            .authorized_mcp_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(list_tools_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| format_streamable_http_error("list GitHub MCP tools", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::ListToolsResult(result) => Ok(result.tools),
            other => Err(PortError::new(format!(
                "GitHub MCP tools/list returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn call_tool_with_session(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
        session_id: Option<Arc<str>>,
    ) -> Result<CallToolResult, PortError> {
        let params = match arguments {
            Some(arguments) => CallToolRequestParams::new(name).with_arguments(arguments),
            None => CallToolRequestParams::new(name),
        };
        let call_tool_request: ClientRequest = CallToolRequest::new(params).into();
        let response = self
            .authorized_mcp_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(call_tool_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| format_streamable_http_error("call GitHub MCP tool", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result),
            other => Err(PortError::new(format!(
                "GitHub MCP tools/call returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn cleanup_session(&self, session_id: Option<Arc<str>>) {
        let Some(session_id) = session_id else {
            return;
        };

        let client = match self.authorized_mcp_client().await {
            Ok(client) => client,
            Err(error) => {
                error!(
                    error = %error.message,
                    "Failed to build authorized GitHub MCP client for session cleanup"
                );
                return;
            }
        };

        if let Err(error) = client
            .delete_session(Arc::clone(&self.uri), session_id, None, HashMap::new())
            .await
        {
            error!(
                error = %format_streamable_http_error("delete GitHub MCP session", error).message,
                "Failed to clean up GitHub MCP session"
            );
        }
    }

    async fn authorized_mcp_client(&self) -> Result<reqwest::Client, PortError> {
        self.auth.authorized_mcp_client().await
    }

    fn next_request_id(&self) -> RequestId {
        NumberOrString::Number(self.request_id.fetch_add(1, Ordering::Relaxed).into())
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
            cached_token: Arc::new(Mutex::new(None)),
        })
    }

    async fn authorized_mcp_client(&self) -> Result<reqwest::Client, PortError> {
        let token = self.installation_token().await?;
        let auth_header = build_bearer_auth_header(token.expose())?;

        build_github_mcp_http_client(auth_header)
    }

    async fn installation_token(&self) -> Result<SecretString, PortError> {
        let now = Utc::now();
        let mut cached_token = self.cached_token.lock().await;
        if let Some(cached_token_value) = cached_token.as_ref()
            && cached_token_value.is_fresh_at(now)
        {
            return Ok(cached_token_value.token.clone());
        }

        let fresh_token = self.request_installation_token().await?;
        let token = fresh_token.token.clone();
        *cached_token = Some(fresh_token);

        Ok(token)
    }

    async fn request_installation_token(&self) -> Result<CachedInstallationToken, PortError> {
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

        Ok(CachedInstallationToken::new(token.token.into(), expires_at))
    }
}

impl CachedInstallationToken {
    fn new(token: SecretString, expires_at: DateTime<Utc>) -> Self {
        Self {
            token,
            refresh_at: expires_at - Duration::minutes(INSTALLATION_TOKEN_REFRESH_SKEW_MINUTES),
        }
    }

    fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        now < self.refresh_at
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

fn build_client_info() -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), build_client_implementation())
}

fn build_client_implementation() -> Implementation {
    Implementation::new(GITHUB_MCP_CLIENT_NAME, GITHUB_MCP_CLIENT_VERSION_FALLBACK)
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

async fn read_server_result(
    response: StreamableHttpPostResponse,
) -> Result<(ServerResult, Option<Arc<str>>), PortError> {
    match response {
        StreamableHttpPostResponse::Accepted => Err(PortError::new(
            "GitHub MCP returned 202 Accepted for a request that required a result",
        )),
        StreamableHttpPostResponse::Json(message, session_id) => {
            Ok((extract_server_result(message)?, session_id.map(Into::into)))
        }
        StreamableHttpPostResponse::Sse(mut stream, session_id) => {
            while let Some(event) = stream.next().await {
                let event = event.map_err(|error| {
                    PortError::new(format!("GitHub MCP SSE stream failed: {error}"))
                })?;
                let payload = event.data.unwrap_or_default();
                if payload.trim().is_empty() {
                    continue;
                }

                let message: ServerJsonRpcMessage =
                    serde_json::from_str(&payload).map_err(|error| {
                        PortError::invalid_response(format!(
                            "Failed to parse GitHub MCP SSE payload: {error}; payload={}",
                            truncate_for_error(&payload)
                        ))
                    })?;

                match message.into_result() {
                    Some((Ok(result), _)) => return Ok((result, session_id.map(Into::into))),
                    Some((Err(error), _)) => {
                        return Err(PortError::new(format!(
                            "GitHub MCP JSON-RPC error: code={:?} message={} data={}",
                            error.code,
                            error.message,
                            error
                                .data
                                .map_or_else(|| "null".to_string(), |value| value.to_string())
                        )));
                    }
                    None => continue,
                }
            }

            Err(PortError::new(format!(
                "GitHub MCP SSE stream ended before a response was received for session {}",
                session_id.as_deref().unwrap_or("<none>")
            )))
        }
        other => Err(PortError::new(format!(
            "GitHub MCP returned an unsupported streamable HTTP response: {other:?}"
        ))),
    }
}

fn extract_server_result(message: ServerJsonRpcMessage) -> Result<ServerResult, PortError> {
    match message.into_result() {
        Some((Ok(result), _)) => Ok(result),
        Some((Err(error), _)) => Err(PortError::new(format!(
            "GitHub MCP JSON-RPC error: code={:?} message={} data={}",
            error.code,
            error.message,
            error
                .data
                .map_or_else(|| "null".to_string(), |value| value.to_string())
        ))),
        None => Err(PortError::new(
            "GitHub MCP returned a notification where a response was expected",
        )),
    }
}

fn format_streamable_http_error(
    context: &str,
    error: StreamableHttpError<reqwest::Error>,
) -> PortError {
    PortError::new(format!("{context} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use futures::{StreamExt, stream};
    use octocrab::models::AppId;
    use reili_core::secret::SecretString;
    use reqwest::header::HeaderValue;
    use rmcp::model::{NumberOrString, ServerJsonRpcMessage, ServerResult};
    use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
    use sse_stream::Sse;

    use super::{
        GITHUB_MCP_CLIENT_NAME, GITHUB_MCP_CLIENT_VERSION_FALLBACK, GITHUB_MCP_TOOLSETS,
        GitHubAppInstallationAuth, GitHubMcpConfig, build_bearer_auth_header,
        build_client_implementation, build_github_mcp_headers, parse_github_app_id,
        read_server_result,
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
            AppId(123)
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
    fn cached_installation_token_refreshes_early() {
        let fresh = super::CachedInstallationToken::new(
            SecretString::from("token"),
            Utc::now() + Duration::minutes(10),
        );
        let stale = super::CachedInstallationToken::new(
            SecretString::from("token"),
            Utc::now() + Duration::minutes(4),
        );

        assert!(fresh.is_fresh_at(Utc::now()));
        assert!(!stale.is_fresh_at(Utc::now()));
    }

    #[test]
    fn builds_client_implementation_without_cargo_pkg_version() {
        let client = build_client_implementation();

        assert_eq!(client.name, GITHUB_MCP_CLIENT_NAME);
        assert_eq!(client.version, GITHUB_MCP_CLIENT_VERSION_FALLBACK);
    }

    #[tokio::test]
    async fn reads_server_result_from_json_response() {
        let response = StreamableHttpPostResponse::Json(
            ServerJsonRpcMessage::response(
                ServerResult::InitializeResult(Default::default()),
                NumberOrString::Number(1.into()),
            ),
            Some("session-123".to_string()),
        );

        let (result, session_id) = read_server_result(response).await.expect("read result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn reads_server_result_from_sse_response() {
        let response = StreamableHttpPostResponse::Sse(
            stream::iter(vec![Ok(Sse::default().data(
                serde_json::to_string(&ServerJsonRpcMessage::response(
                    ServerResult::InitializeResult(Default::default()),
                    NumberOrString::Number(1.into()),
                ))
                .expect("serialize json-rpc response"),
            ))])
            .boxed(),
            Some("session-123".to_string()),
        );

        let (result, session_id) = read_server_result(response).await.expect("read sse result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn skips_empty_sse_events_until_response_arrives() {
        let response = StreamableHttpPostResponse::Sse(
            stream::iter(vec![
                Ok(Sse::default().data("   ")),
                Ok(Sse::default().data(
                    serde_json::to_string(&ServerJsonRpcMessage::response(
                        ServerResult::InitializeResult(Default::default()),
                        NumberOrString::Number(1.into()),
                    ))
                    .expect("serialize response"),
                )),
            ])
            .boxed(),
            Some("session-123".to_string()),
        );

        let (result, session_id) = read_server_result(response).await.expect("read sse result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }
}
