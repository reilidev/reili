use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use reili_core::error::PortError;
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
use tracing::{debug, error};

const MCP_CLIENT_NAME: &str = "reili";
const MCP_CLIENT_VERSION_FALLBACK: &str = "unknown";
// ~500 chars: keeps malformed/oversized SSE payloads readable in error messages and logs.
const MAX_MCP_ERROR_BODY_CHARS: usize = 500;

/// Static for Datadog/JIRA; GitHub refreshes and rebuilds the client only when its token expires.
#[async_trait]
pub(crate) trait McpHttpClientAuth: Send + Sync {
    async fn http_client(&self) -> Result<reqwest::Client, PortError>;
}

#[derive(Clone)]
pub(crate) struct StaticMcpAuth(pub(crate) reqwest::Client);

#[async_trait]
impl McpHttpClientAuth for StaticMcpAuth {
    async fn http_client(&self) -> Result<reqwest::Client, PortError> {
        Ok(self.0.clone())
    }
}

/// Session lifecycle and streamable-HTTP response handling shared by every rmcp-backed connector.
#[derive(Clone)]
pub(crate) struct StreamableHttpMcpClient<A: McpHttpClientAuth + Clone> {
    source_label: &'static str,
    uri: Arc<str>,
    client_info: ClientInfo,
    request_id: Arc<AtomicU32>,
    auth: A,
}

impl<A: McpHttpClientAuth + Clone> StreamableHttpMcpClient<A> {
    pub(crate) fn new(source_label: &'static str, uri: impl Into<Arc<str>>, auth: A) -> Self {
        Self {
            source_label,
            uri: uri.into(),
            client_info: build_client_info(),
            request_id: Arc::new(AtomicU32::new(1)),
            auth,
        }
    }

    pub(crate) async fn list_tools(&self) -> Result<Vec<Tool>, PortError> {
        let session_id = self.initialize_session().await?;
        let result = self.list_tools_with_session(session_id.clone()).await;
        self.cleanup_session(session_id).await;
        result
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

    async fn http_client(&self) -> Result<reqwest::Client, PortError> {
        self.auth.http_client().await
    }

    async fn initialize_session(&self) -> Result<Option<Arc<str>>, PortError> {
        let initialize_request: ClientRequest =
            InitializeRequest::new(self.client_info.clone()).into();
        let initialize_response = self
            .http_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(initialize_request, self.next_request_id()),
                None,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| self.format_streamable_http_error("initialize session", error))?;
        let (initialize_result, session_id) = self.read_server_result(initialize_response).await?;
        match initialize_result {
            ServerResult::InitializeResult(_) => {}
            other => {
                return Err(PortError::new(format!(
                    "{} MCP initialize returned unexpected result: {other:?}",
                    self.source_label
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
        self.http_client()
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
                self.format_streamable_http_error("send initialized notification", error)
            })?
            .expect_accepted_or_json::<reqwest::Error>()
            .map_err(|error| {
                self.format_streamable_http_error(
                    "process initialized notification response",
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
            .http_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(list_tools_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| self.format_streamable_http_error("list tools", error))?;
        let (result, _) = self.read_server_result(response).await?;
        match result {
            ServerResult::ListToolsResult(result) => Ok(result.tools),
            other => Err(PortError::new(format!(
                "{} MCP tools/list returned unexpected result: {other:?}",
                self.source_label
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
            .http_client()
            .await?
            .post_message(
                Arc::clone(&self.uri),
                ClientJsonRpcMessage::request(call_tool_request, self.next_request_id()),
                session_id,
                None,
                HashMap::new(),
            )
            .await
            .map_err(|error| self.format_streamable_http_error("call tool", error))?;
        let (result, _) = self.read_server_result(response).await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result),
            other => Err(PortError::new(format!(
                "{} MCP tools/call returned unexpected result: {other:?}",
                self.source_label
            ))),
        }
    }

    async fn cleanup_session(&self, session_id: Option<Arc<str>>) {
        let Some(session_id) = session_id else {
            return;
        };

        let client = match self.http_client().await {
            Ok(client) => client,
            Err(error) => {
                error!(
                    error = %error.message,
                    source = self.source_label,
                    "Failed to build authorized MCP client for session cleanup"
                );
                return;
            }
        };

        if let Err(error) = client
            .delete_session(Arc::clone(&self.uri), session_id, None, HashMap::new())
            .await
        {
            error!(
                error = %self.format_streamable_http_error("delete session", error).message,
                "Failed to clean up MCP session"
            );
        }
    }

    fn next_request_id(&self) -> RequestId {
        NumberOrString::Number(self.request_id.fetch_add(1, Ordering::Relaxed).into())
    }

    async fn read_server_result(
        &self,
        response: StreamableHttpPostResponse,
    ) -> Result<(ServerResult, Option<Arc<str>>), PortError> {
        match response {
            StreamableHttpPostResponse::Accepted => Err(PortError::new(format!(
                "{} MCP returned 202 Accepted for a request that required a result",
                self.source_label
            ))),
            StreamableHttpPostResponse::Json(message, session_id) => Ok((
                self.extract_server_result(message)?,
                session_id.map(Into::into),
            )),
            StreamableHttpPostResponse::Sse(mut stream, session_id) => {
                // Streamable HTTP servers may answer any request with SSE instead of plain JSON.
                while let Some(event) = stream.next().await {
                    let event = event.map_err(|error| {
                        PortError::new(format!(
                            "{} MCP SSE stream failed: {error}",
                            self.source_label
                        ))
                    })?;
                    let payload = event.data.unwrap_or_default();
                    if payload.trim().is_empty() {
                        continue;
                    }

                    let message: ServerJsonRpcMessage =
                        serde_json::from_str(&payload).map_err(|error| {
                            PortError::invalid_response(format!(
                                "Failed to parse {} MCP SSE payload: {error}; payload={}",
                                self.source_label,
                                truncate_for_error(&payload)
                            ))
                        })?;

                    match message.clone().into_result() {
                        Some((Ok(result), _)) => {
                            return Ok((result, session_id.map(Into::into)));
                        }
                        Some((Err(error), _)) => {
                            return Err(PortError::new(format!(
                                "{} MCP JSON-RPC error: code={:?} message={} data={}",
                                self.source_label,
                                error.code,
                                error.message,
                                error
                                    .data
                                    .map_or_else(|| "null".to_string(), |value| value.to_string())
                            )));
                        }
                        None => {
                            debug!(
                                ?message,
                                source = self.source_label,
                                "MCP SSE stream emitted a message before the result; continuing to drain"
                            );
                        }
                    }
                }

                Err(PortError::new(format!(
                    "{} MCP SSE stream ended for session {} without returning a result",
                    self.source_label,
                    session_id.as_deref().unwrap_or("<none>")
                )))
            }
            other => Err(PortError::new(format!(
                "{} MCP returned an unsupported streamable HTTP response: {other:?}",
                self.source_label
            ))),
        }
    }

    fn extract_server_result(
        &self,
        message: ServerJsonRpcMessage,
    ) -> Result<ServerResult, PortError> {
        match message.into_result() {
            Some((Ok(result), _)) => Ok(result),
            Some((Err(error), _)) => Err(PortError::new(format!(
                "{} MCP JSON-RPC error: code={:?} message={} data={}",
                self.source_label,
                error.code,
                error.message,
                error
                    .data
                    .map_or_else(|| "null".to_string(), |value| value.to_string())
            ))),
            None => Err(PortError::new(format!(
                "{} MCP returned a notification where a response was expected",
                self.source_label
            ))),
        }
    }

    fn format_streamable_http_error(
        &self,
        context: &str,
        error: StreamableHttpError<reqwest::Error>,
    ) -> PortError {
        PortError::new(format!(
            "{} MCP {context} failed: {error}",
            self.source_label
        ))
    }
}

pub(crate) fn build_client_info() -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), build_client_implementation())
}

pub(crate) fn build_client_implementation() -> Implementation {
    Implementation::new(MCP_CLIENT_NAME, MCP_CLIENT_VERSION_FALLBACK)
}

pub(crate) fn truncate_for_error(value: &str) -> String {
    let mut truncated = value
        .chars()
        .take(MAX_MCP_ERROR_BODY_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_MCP_ERROR_BODY_CHARS {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use rmcp::model::{NumberOrString, ServerJsonRpcMessage, ServerResult};
    use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;
    use sse_stream::Sse;

    use super::{
        MCP_CLIENT_NAME, MCP_CLIENT_VERSION_FALLBACK, StaticMcpAuth, StreamableHttpMcpClient,
        build_client_implementation,
    };

    fn client() -> StreamableHttpMcpClient<StaticMcpAuth> {
        StreamableHttpMcpClient::new(
            "Example",
            "https://example.invalid/mcp",
            StaticMcpAuth(reqwest::Client::new()),
        )
    }

    #[test]
    fn builds_client_implementation_without_cargo_pkg_version() {
        let implementation = build_client_implementation();

        assert_eq!(implementation.name, MCP_CLIENT_NAME);
        assert_eq!(implementation.version, MCP_CLIENT_VERSION_FALLBACK);
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

        let (result, session_id) = client()
            .read_server_result(response)
            .await
            .expect("read result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn reads_server_result_from_sse_response() {
        let message = ServerJsonRpcMessage::response(
            ServerResult::InitializeResult(Default::default()),
            NumberOrString::Number(1.into()),
        );
        let event = Sse::default().data(serde_json::to_string(&message).expect("serialize"));
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::iter([Ok(event)]).boxed(),
            Some("session-123".to_string()),
        );

        let (result, session_id) = client()
            .read_server_result(response)
            .await
            .expect("read result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn skips_blank_sse_events_before_returning_result() {
        let message = ServerJsonRpcMessage::response(
            ServerResult::InitializeResult(Default::default()),
            NumberOrString::Number(1.into()),
        );
        let events: Vec<Result<Sse, sse_stream::Error>> = vec![
            Ok(Sse::default()),
            Ok(Sse::default().data(serde_json::to_string(&message).expect("serialize"))),
        ];
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::iter(events).boxed(),
            Some("session-123".to_string()),
        );

        let (result, session_id) = client()
            .read_server_result(response)
            .await
            .expect("read result");

        assert!(matches!(result, ServerResult::InitializeResult(_)));
        assert_eq!(session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn surfaces_json_rpc_error_delivered_over_sse() {
        let event = Sse::default().data(
            serde_json::to_string(&ServerJsonRpcMessage::error(
                rmcp::model::ErrorData::internal_error("boom", None),
                Some(NumberOrString::Number(1.into())),
            ))
            .expect("serialize"),
        );
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::iter([Ok(event)]).boxed(),
            Some("session-123".to_string()),
        );

        let error = client()
            .read_server_result(response)
            .await
            .expect_err("json-rpc error should surface");

        assert!(error.message.contains("Example MCP JSON-RPC error"));
        assert!(error.message.contains("boom"));
    }

    #[tokio::test]
    async fn rejects_empty_sse_stream_without_result() {
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::empty().boxed(),
            Some("session-123".to_string()),
        );

        let error = client()
            .read_server_result(response)
            .await
            .expect_err("empty sse stream should fail");

        assert_eq!(
            error.message,
            "Example MCP SSE stream ended for session session-123 without returning a result"
        );
    }
}
