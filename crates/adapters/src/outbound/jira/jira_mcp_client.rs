use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::StreamExt;
use reili_core::error::PortError;
use reili_core::secret::SecretString;
use reqwest::header::{AUTHORIZATION, HeaderMap as ReqwestHeaderMap, HeaderValue};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientJsonRpcMessage, ClientNotification, ClientRequest, Implementation, InitializeRequest,
    InitializedNotification, ListToolsRequest, NumberOrString, RequestId, ServerJsonRpcMessage,
    ServerResult, Tool,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use tracing::error;

const JIRA_MCP_CLIENT_NAME: &str = "reili";
const JIRA_MCP_CLIENT_VERSION_FALLBACK: &str = "unknown";
// About ~500 chars; keeps malformed SSE payloads out of error messages/logs at a readable length.
const MAX_ERROR_BODY_CHARS: usize = 500;
/// Atlassian's Rovo MCP server. One fixed endpoint serves every Cloud tenant; the target site is
/// selected per tool call via the `cloudId` argument, not the connection URL.
const ROVO_MCP_URL: &str = "https://mcp.atlassian.com/v1/mcp";

#[derive(Clone, PartialEq, Eq)]
pub struct JiraMcpConfig {
    /// Atlassian Cloud site hostname, e.g. `acme.atlassian.net`. Used only as the `cloudId`
    /// argument stamped onto every tool call, not as part of the connection URL.
    pub site: String,
    pub service_account_api_token: SecretString,
}

impl std::fmt::Debug for JiraMcpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraMcpConfig")
            .field("site", &self.site)
            .field("service_account_api_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct JiraMcpHttpClient {
    http_client: reqwest::Client,
    uri: Arc<str>,
    client_info: ClientInfo,
    request_id: Arc<AtomicU32>,
}

impl JiraMcpHttpClient {
    pub(crate) async fn connect(config: &JiraMcpConfig) -> Result<(Self, Vec<Tool>), PortError> {
        let client = Self {
            http_client: build_jira_mcp_http_client(config)?,
            uri: ROVO_MCP_URL.into(),
            client_info: build_client_info(),
            request_id: Arc::new(AtomicU32::new(1)),
        };
        let tools = client.list_tools().await.map_err(|error| {
            PortError::connection_failed(format!(
                "Failed to connect to JIRA (Atlassian Rovo) MCP server: {}",
                error.message
            ))
        })?;

        Ok((client, tools))
    }

    pub(crate) async fn call_tool(
        &self,
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
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
            .http_client
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(initialize_request, self.next_request_id()),
                None,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| format_streamable_http_error("initialize JIRA MCP session", error))?;
        let (initialize_result, session_id) = read_server_result(initialize_response).await?;
        match initialize_result {
            ServerResult::InitializeResult(_) => {}
            other => {
                return Err(PortError::new(format!(
                    "JIRA MCP initialize returned unexpected result: {other:?}"
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
        self.http_client
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::notification(notification),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| {
                format_streamable_http_error("send initialized notification to JIRA MCP", error)
            })?
            .expect_accepted_or_json::<reqwest::Error>()
            .map_err(|error| {
                format_streamable_http_error(
                    "process initialized notification response from JIRA MCP",
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
            .http_client
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(list_tools_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| format_streamable_http_error("list JIRA MCP tools", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::ListToolsResult(result) => Ok(result.tools),
            other => Err(PortError::new(format!(
                "JIRA MCP tools/list returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn call_tool_with_session(
        &self,
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        session_id: Option<Arc<str>>,
    ) -> Result<CallToolResult, PortError> {
        let params = match arguments {
            Some(arguments) => CallToolRequestParams::new(name).with_arguments(arguments),
            None => CallToolRequestParams::new(name),
        };
        let call_tool_request: ClientRequest = CallToolRequest::new(params).into();
        let response = self
            .http_client
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(call_tool_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| format_streamable_http_error("call JIRA MCP tool", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result),
            other => Err(PortError::new(format!(
                "JIRA MCP tools/call returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn cleanup_session(&self, session_id: Option<Arc<str>>) {
        let Some(session_id) = session_id else {
            return;
        };

        if let Err(error) = self
            .http_client
            .delete_session(Arc::clone(&self.uri), session_id, None, HashMap::new())
            .await
        {
            error!(
                error = %format_streamable_http_error("delete JIRA MCP session", error).message,
                "Failed to clean up JIRA MCP session"
            );
        }
    }

    fn next_request_id(&self) -> RequestId {
        NumberOrString::Number(self.request_id.fetch_add(1, Ordering::Relaxed).into())
    }
}

fn build_client_info() -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), build_client_implementation())
}

fn build_client_implementation() -> Implementation {
    Implementation::new(JIRA_MCP_CLIENT_NAME, JIRA_MCP_CLIENT_VERSION_FALLBACK)
}

fn build_jira_mcp_http_client(config: &JiraMcpConfig) -> Result<reqwest::Client, PortError> {
    reqwest::Client::builder()
        .default_headers(build_jira_mcp_headers(config)?)
        .build()
        .map_err(|error| {
            PortError::new(format!(
                "Failed to build JIRA MCP HTTP client with default headers: {error}"
            ))
        })
}

fn build_jira_mcp_headers(config: &JiraMcpConfig) -> Result<ReqwestHeaderMap, PortError> {
    let mut default_headers = ReqwestHeaderMap::new();
    default_headers.insert(AUTHORIZATION, build_bearer_auth_header(config)?);

    Ok(default_headers)
}

fn build_bearer_auth_header(config: &JiraMcpConfig) -> Result<HeaderValue, PortError> {
    let token = config.service_account_api_token.expose().trim();
    let header_value = if token.starts_with("Bearer ") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    };

    HeaderValue::from_str(&header_value)
        .map_err(|error| PortError::new(format!("Invalid JIRA MCP authorization header: {error}")))
}

async fn read_server_result(
    response: StreamableHttpPostResponse,
) -> Result<(ServerResult, Option<Arc<str>>), PortError> {
    match response {
        StreamableHttpPostResponse::Accepted => Err(PortError::new(
            "JIRA MCP returned 202 Accepted for a request that required a result",
        )),
        StreamableHttpPostResponse::Json(message, session_id) => {
            Ok((extract_server_result(message)?, session_id.map(Into::into)))
        }
        StreamableHttpPostResponse::Sse(mut stream, session_id) => {
            while let Some(event) = stream.next().await {
                let event = event.map_err(|error| {
                    PortError::new(format!("JIRA MCP SSE stream failed: {error}"))
                })?;
                let payload = event.data.unwrap_or_default();
                if payload.trim().is_empty() {
                    continue;
                }

                let message: ServerJsonRpcMessage =
                    serde_json::from_str(&payload).map_err(|error| {
                        PortError::invalid_response(format!(
                            "Failed to parse JIRA MCP SSE payload: {error}; payload={}",
                            truncate_for_error(&payload)
                        ))
                    })?;

                match message.into_result() {
                    Some((Ok(result), _)) => return Ok((result, session_id.map(Into::into))),
                    Some((Err(error), _)) => {
                        return Err(PortError::new(format!(
                            "JIRA MCP JSON-RPC error: code={:?} message={} data={}",
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
                "JIRA MCP SSE stream ended before a response was received for session {}",
                session_id.as_deref().unwrap_or("<none>")
            )))
        }
        other => Err(PortError::new(format!(
            "JIRA MCP returned an unsupported streamable HTTP response: {other:?}"
        ))),
    }
}

fn truncate_for_error(value: &str) -> String {
    let mut truncated = value.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>();
    if value.chars().count() > MAX_ERROR_BODY_CHARS {
        truncated.push_str("...");
    }
    truncated
}

fn extract_server_result(message: ServerJsonRpcMessage) -> Result<ServerResult, PortError> {
    match message.into_result() {
        Some((Ok(result), _)) => Ok(result),
        Some((Err(error), _)) => Err(PortError::new(format!(
            "JIRA MCP JSON-RPC error: code={:?} message={} data={}",
            error.code,
            error.message,
            error
                .data
                .map_or_else(|| "null".to_string(), |value| value.to_string())
        ))),
        None => Err(PortError::new(
            "JIRA MCP returned a notification where a response was expected",
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
    use futures::StreamExt;
    use reqwest::header::{AUTHORIZATION, HeaderValue};
    use rmcp::model::{NumberOrString, ServerJsonRpcMessage, ServerResult};
    use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
    use sse_stream::Sse;

    use super::{
        JIRA_MCP_CLIENT_NAME, JIRA_MCP_CLIENT_VERSION_FALLBACK, JiraMcpConfig,
        build_client_implementation, build_jira_mcp_headers, read_server_result,
    };
    use reili_core::secret::SecretString;

    fn config(token: &str) -> JiraMcpConfig {
        JiraMcpConfig {
            site: "acme.atlassian.net".to_string(),
            service_account_api_token: SecretString::from(token),
        }
    }

    #[test]
    fn builds_bearer_authorization_header_when_token_has_no_prefix() {
        let headers = build_jira_mcp_headers(&config("api-key")).expect("build headers");

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer api-key"))
        );
    }

    #[test]
    fn does_not_double_prefix_bearer_token() {
        let headers = build_jira_mcp_headers(&config("Bearer api-key")).expect("build headers");

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer api-key"))
        );
    }

    #[test]
    fn debug_redacts_service_account_api_token() {
        let debug_output = format!("{:?}", config("super-secret"));

        assert!(!debug_output.contains("super-secret"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn builds_client_implementation_without_cargo_pkg_version() {
        let client = build_client_implementation();

        assert_eq!(client.name, JIRA_MCP_CLIENT_NAME);
        assert_eq!(client.version, JIRA_MCP_CLIENT_VERSION_FALLBACK);
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
            futures::stream::iter(vec![Ok(Sse::default().data(
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
            futures::stream::iter(vec![
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

    #[tokio::test]
    async fn rejects_sse_stream_that_ends_without_a_response() {
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::empty().boxed(),
            Some("session-123".to_string()),
        );

        let error = read_server_result(response)
            .await
            .expect_err("empty sse stream should fail");

        assert_eq!(
            error.message,
            "JIRA MCP SSE stream ended before a response was received for session session-123"
        );
    }
}
