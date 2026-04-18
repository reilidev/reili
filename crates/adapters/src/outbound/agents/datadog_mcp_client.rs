use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use reili_core::error::PortError;
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientJsonRpcMessage, ClientNotification, ClientRequest, Implementation, InitializeRequest,
    InitializedNotification, ListToolsRequest, NumberOrString, RequestId, ServerJsonRpcMessage,
    ServerResult, Tool,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::json;
use tracing::error;

const DATADOG_MCP_CLIENT_NAME: &str = "reili";
const DATADOG_MCP_CLIENT_VERSION_FALLBACK: &str = "unknown";
const DATADOG_MCP_TOOLSETS: &str = "core,security";
const DATADOG_API_KEY_HEADER: &str = "DD_API_KEY";
const DATADOG_APPLICATION_KEY_HEADER: &str = "DD_APPLICATION_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatadogMcpToolConfig {
    pub api_key: String,
    pub app_key: String,
    pub site: String,
}

#[derive(Clone)]
pub(super) struct DatadogMcpHttpClient {
    http_client: reqwest::Client,
    uri: Arc<str>,
    client_info: ClientInfo,
    request_id: Arc<AtomicU32>,
}

impl DatadogMcpHttpClient {
    pub(super) async fn connect(
        config: &DatadogMcpToolConfig,
    ) -> Result<(Self, Vec<Tool>), PortError> {
        let client = Self {
            http_client: build_datadog_mcp_http_client(config)?,
            uri: datadog_mcp_url(&config.site).into(),
            client_info: build_client_info(),
            request_id: Arc::new(AtomicU32::new(1)),
        };
        let tools = match client.list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                let diagnostic = diagnose_datadog_mcp_initialize(config).await;
                return Err(create_datadog_mcp_connect_error(error.message, diagnostic));
            }
        };

        Ok((client, tools))
    }

    pub(super) async fn call_tool(
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
        let initialize_request: ClientRequest = InitializeRequest {
            method: Default::default(),
            params: self.client_info.clone(),
            extensions: Default::default(),
        }
        .into();
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
            .map_err(|error| {
                format_streamable_http_error("initialize Datadog MCP session", error)
            })?;
        let (initialize_result, session_id) = read_server_result(initialize_response).await?;
        match initialize_result {
            ServerResult::InitializeResult(_) => {}
            other => {
                return Err(PortError::new(format!(
                    "Datadog MCP initialize returned unexpected result: {other:?}"
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
                format_streamable_http_error("send initialized notification to Datadog MCP", error)
            })?
            .expect_accepted::<reqwest::Error>()
            .map_err(|error| {
                format_streamable_http_error(
                    "process initialized notification response from Datadog MCP",
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
            .map_err(|error| format_streamable_http_error("list Datadog MCP tools", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::ListToolsResult(result) => Ok(result.tools),
            other => Err(PortError::new(format!(
                "Datadog MCP tools/list returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn call_tool_with_session(
        &self,
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        session_id: Option<Arc<str>>,
    ) -> Result<CallToolResult, PortError> {
        let call_tool_request: ClientRequest = CallToolRequest {
            method: Default::default(),
            params: CallToolRequestParams {
                name: name.into(),
                arguments,
                meta: None,
                task: None,
            },
            extensions: Default::default(),
        }
        .into();
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
            .map_err(|error| format_streamable_http_error("call Datadog MCP tool", error))?;
        let (result, _) = read_server_result(response).await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result),
            other => Err(PortError::new(format!(
                "Datadog MCP tools/call returned unexpected result: {other:?}"
            ))),
        }
    }

    async fn cleanup_session(&self, session_id: Option<Arc<str>>) {
        let Some(session_id) = session_id else {
            return;
        };

        if let Err(error) = self
            .http_client
            .delete_session(Arc::clone(&self.uri), session_id, None)
            .await
        {
            error!(
                error = %format_streamable_http_error("delete Datadog MCP session", error).message,
                "Failed to clean up Datadog MCP session"
            );
        }
    }

    fn next_request_id(&self) -> RequestId {
        NumberOrString::Number(self.request_id.fetch_add(1, Ordering::Relaxed).into())
    }
}

fn build_client_info() -> ClientInfo {
    ClientInfo {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: build_client_implementation(),
        meta: None,
    }
}

fn build_client_implementation() -> Implementation {
    Implementation {
        name: DATADOG_MCP_CLIENT_NAME.to_string(),
        version: DATADOG_MCP_CLIENT_VERSION_FALLBACK.to_string(),
        ..Default::default()
    }
}

fn build_datadog_mcp_http_client(
    config: &DatadogMcpToolConfig,
) -> Result<reqwest::Client, PortError> {
    reqwest::Client::builder()
        .default_headers(build_datadog_mcp_headers(config)?)
        .build()
        .map_err(|error| {
            PortError::new(format!(
                "Failed to build Datadog MCP HTTP client with default headers: {error}"
            ))
        })
}

async fn diagnose_datadog_mcp_initialize(
    config: &DatadogMcpToolConfig,
) -> Result<DatadogMcpInitializeDiagnostic, PortError> {
    let client_implementation = build_client_implementation();
    let response = reqwest::Client::new()
        .post(datadog_mcp_url(&config.site))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header(DATADOG_API_KEY_HEADER, &config.api_key)
        .header(DATADOG_APPLICATION_KEY_HEADER, &config.app_key)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": client_implementation.name,
                    "version": client_implementation.version,
                }
            }
        }))
        .send()
        .await
        .map_err(|error| {
            PortError::new(format!(
                "Failed to run Datadog MCP initialize diagnostic request: {error}"
            ))
        })?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = response.text().await.map_err(|error| {
        PortError::new(format!(
            "Failed to read Datadog MCP initialize diagnostic response body: {error}"
        ))
    })?;

    Ok(DatadogMcpInitializeDiagnostic {
        status,
        content_type,
        body,
    })
}

fn create_datadog_mcp_connect_error(
    base_error: String,
    diagnostic: Result<DatadogMcpInitializeDiagnostic, PortError>,
) -> PortError {
    match diagnostic {
        Ok(diagnostic) => PortError::connection_failed(format!(
            "Failed to connect to Datadog MCP server: {base_error}. Diagnostic initialize response: status={} content_type={} body={}",
            diagnostic.status, diagnostic.content_type, diagnostic.body
        )),
        Err(diagnostic_error) => PortError::connection_failed(format!(
            "Failed to connect to Datadog MCP server: {base_error}. Diagnostic request also failed: {}",
            diagnostic_error.message
        )),
    }
}

fn build_datadog_mcp_headers(config: &DatadogMcpToolConfig) -> Result<ReqwestHeaderMap, PortError> {
    let mut default_headers = ReqwestHeaderMap::new();
    default_headers.insert(
        HeaderName::from_static("dd_api_key"),
        HeaderValue::from_str(&config.api_key)
            .map_err(|error| PortError::new(format!("Invalid Datadog API key header: {error}")))?,
    );
    default_headers.insert(
        HeaderName::from_static("dd_application_key"),
        HeaderValue::from_str(&config.app_key).map_err(|error| {
            PortError::new(format!("Invalid Datadog application key header: {error}"))
        })?,
    );

    Ok(default_headers)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DatadogMcpInitializeDiagnostic {
    status: u16,
    content_type: String,
    body: String,
}

async fn read_server_result(
    response: StreamableHttpPostResponse,
) -> Result<(ServerResult, Option<Arc<str>>), PortError> {
    match response {
        StreamableHttpPostResponse::Accepted => Err(PortError::new(
            "Datadog MCP returned 202 Accepted for a request that required a result",
        )),
        StreamableHttpPostResponse::Json(message, session_id) => {
            Ok((extract_server_result(message)?, session_id.map(Into::into)))
        }
        StreamableHttpPostResponse::Sse(_, session_id) => Err(PortError::new(format!(
            "Datadog MCP returned an unexpected SSE response for session {}",
            session_id.as_deref().unwrap_or("<none>")
        ))),
    }
}

fn extract_server_result(message: ServerJsonRpcMessage) -> Result<ServerResult, PortError> {
    match message.into_result() {
        Some((Ok(result), _)) => Ok(result),
        Some((Err(error), _)) => Err(PortError::new(format!(
            "Datadog MCP JSON-RPC error: code={:?} message={} data={}",
            error.code,
            error.message,
            error
                .data
                .map_or_else(|| "null".to_string(), |value| value.to_string())
        ))),
        None => Err(PortError::new(
            "Datadog MCP returned a notification where a response was expected",
        )),
    }
}

fn format_streamable_http_error(
    context: &str,
    error: StreamableHttpError<reqwest::Error>,
) -> PortError {
    PortError::new(format!("{context} failed: {error}"))
}

fn datadog_mcp_url(site: &str) -> String {
    let site_domain = datadog_site_domain(site);

    format!("https://mcp.{site_domain}/api/unstable/mcp-server/mcp?toolsets={DATADOG_MCP_TOOLSETS}")
}

fn datadog_site_domain(site: &str) -> &str {
    let site = site.trim();
    let site = site
        .strip_prefix("https://")
        .or_else(|| site.strip_prefix("http://"))
        .unwrap_or(site);

    site.split('/').next().unwrap_or(site)
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use reqwest::header::HeaderValue;
    use rmcp::model::{NumberOrString, ServerJsonRpcMessage, ServerResult};
    use rmcp::transport::streamable_http_client::StreamableHttpPostResponse;

    use super::{
        DATADOG_MCP_CLIENT_NAME, DATADOG_MCP_CLIENT_VERSION_FALLBACK, DatadogMcpToolConfig,
        build_client_implementation, build_datadog_mcp_headers, datadog_mcp_url,
        datadog_site_domain, read_server_result,
    };

    #[test]
    fn builds_datadog_mcp_url_from_site() {
        assert_eq!(
            datadog_mcp_url("datadoghq.eu"),
            "https://mcp.datadoghq.eu/api/unstable/mcp-server/mcp?toolsets=core,security"
        );
        assert_eq!(
            datadog_mcp_url("ap1.datadoghq.com"),
            "https://mcp.ap1.datadoghq.com/api/unstable/mcp-server/mcp?toolsets=core,security"
        );
    }

    #[test]
    fn extracts_domain_from_datadog_site_env_value() {
        assert_eq!(
            datadog_site_domain("ap1.datadoghq.com"),
            "ap1.datadoghq.com"
        );
        assert_eq!(
            datadog_site_domain("https://ap1.datadoghq.com"),
            "ap1.datadoghq.com"
        );
        assert_eq!(
            datadog_site_domain("https://ap1.datadoghq.com/"),
            "ap1.datadoghq.com"
        );
    }

    #[test]
    fn builds_datadog_mcp_headers_with_underscore_header_names() {
        let headers = build_datadog_mcp_headers(&DatadogMcpToolConfig {
            api_key: "api-key".to_string(),
            app_key: "app-key".to_string(),
            site: "datadoghq.com".to_string(),
        })
        .expect("build headers");

        assert_eq!(
            headers.get("dd_api_key"),
            Some(&HeaderValue::from_static("api-key"))
        );
        assert_eq!(
            headers.get("dd_application_key"),
            Some(&HeaderValue::from_static("app-key"))
        );
        assert!(headers.get("dd-api-key").is_none());
        assert!(headers.get("dd-application-key").is_none());
    }

    #[test]
    fn builds_client_implementation_without_cargo_pkg_version() {
        let client = build_client_implementation();

        assert_eq!(client.name, DATADOG_MCP_CLIENT_NAME);
        assert_eq!(client.version, DATADOG_MCP_CLIENT_VERSION_FALLBACK);
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
    async fn rejects_sse_server_result_response() {
        let response = StreamableHttpPostResponse::Sse(
            futures::stream::empty().boxed(),
            Some("session-123".to_string()),
        );

        let error = read_server_result(response)
            .await
            .expect_err("sse should fail");

        assert_eq!(
            error.message,
            "Datadog MCP returned an unexpected SSE response for session session-123"
        );
    }
}
