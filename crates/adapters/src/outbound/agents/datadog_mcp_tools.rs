use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::StreamExt;
use reili_core::error::PortError;
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderName, HeaderValue};
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientJsonRpcMessage, ClientNotification, ClientRequest, Content, Implementation,
    InitializeRequest, InitializedNotification, ListToolsRequest, NumberOrString, RequestId,
    ServerJsonRpcMessage, ServerResult, Tool,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use serde_json::json;
use tracing::error;

const DATADOG_MCP_CLIENT_NAME: &str = "reili";
const DATADOG_MCP_TOOLSET: &str = "core";
const DATADOG_API_KEY_HEADER: &str = "DD_API_KEY";
const DATADOG_APPLICATION_KEY_HEADER: &str = "DD_APPLICATION_KEY";

const DATADOG_LOGS_AGENT_TOOLS: &[&str] = &["search_datadog_logs", "analyze_datadog_logs"];
const DATADOG_METRICS_AGENT_TOOLS: &[&str] = &[
    "search_datadog_metrics",
    "get_datadog_metric",
    "get_datadog_metric_context",
];
const DATADOG_EVENTS_AGENT_TOOLS: &[&str] = &["search_datadog_events"];
const DATADOG_LEAD_AGENT_TOOLS: &[&str] = &[
    "search_datadog_services",
    "search_datadog_metrics",
    "get_datadog_metric_context",
    "search_datadog_monitors",
    "search_datadog_incidents",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatadogMcpToolConfig {
    pub api_key: String,
    pub app_key: String,
    pub site: String,
}

#[derive(Clone)]
struct DatadogMcpHttpClient {
    http_client: reqwest::Client,
    uri: Arc<str>,
    client_info: ClientInfo,
    request_id: Arc<AtomicU32>,
}

#[derive(Clone)]
pub struct DatadogMcpToolset {
    tools: Vec<Tool>,
    client: DatadogMcpHttpClient,
}

impl DatadogMcpToolset {
    #[must_use]
    pub fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_rig_tools(&self.tools, DATADOG_LEAD_AGENT_TOOLS, self.client.clone())
    }

    #[must_use]
    pub fn logs_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_rig_tools(&self.tools, DATADOG_LOGS_AGENT_TOOLS, self.client.clone())
    }

    #[must_use]
    pub fn metrics_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_rig_tools(
            &self.tools,
            DATADOG_METRICS_AGENT_TOOLS,
            self.client.clone(),
        )
    }

    #[must_use]
    pub fn events_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_rig_tools(&self.tools, DATADOG_EVENTS_AGENT_TOOLS, self.client.clone())
    }
}

pub async fn connect_datadog_mcp_toolset(
    config: &DatadogMcpToolConfig,
) -> Result<DatadogMcpToolset, PortError> {
    let client_info = ClientInfo {
        protocol_version: Default::default(),
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: DATADOG_MCP_CLIENT_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        },
        meta: None,
    };
    let client = DatadogMcpHttpClient {
        http_client: build_datadog_mcp_http_client(config)?,
        uri: datadog_mcp_url(&config.site).into(),
        client_info,
        request_id: Arc::new(AtomicU32::new(1)),
    };
    let tools = match client.list_tools().await {
        Ok(tools) => tools,
        Err(error) => {
            println!("Failed to connect to Datadog MCP server: {}", error.message);
            let diagnostic = diagnose_datadog_mcp_initialize(config).await;
            return Err(create_datadog_mcp_connect_error(error.message, diagnostic));
        }
    };

    validate_required_tools(&tools)?;

    Ok(DatadogMcpToolset { tools, client })
}

impl DatadogMcpHttpClient {
    async fn list_tools(&self) -> Result<Vec<Tool>, PortError> {
        let session_id = self.initialize_session().await?;
        let result = self.list_tools_with_session(session_id.clone()).await;
        self.cleanup_session(session_id).await;
        result
    }

    async fn call_tool(
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
                    "name": DATADOG_MCP_CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
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
        body_preview: truncate_for_error_message(&body, 400),
    })
}

fn create_datadog_mcp_connect_error(
    base_error: String,
    diagnostic: Result<DatadogMcpInitializeDiagnostic, PortError>,
) -> PortError {
    match diagnostic {
        Ok(diagnostic) => PortError::new(format!(
            "Failed to connect to Datadog MCP server: {base_error}. Diagnostic initialize response: status={} content_type={} body={}",
            diagnostic.status, diagnostic.content_type, diagnostic.body_preview
        )),
        Err(diagnostic_error) => PortError::new(format!(
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

fn truncate_for_error_message(text: &str, max_chars: usize) -> String {
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        truncated.push_str("...");
    }

    truncated
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DatadogMcpInitializeDiagnostic {
    status: u16,
    content_type: String,
    body_preview: String,
}

fn validate_required_tools(tools: &[Tool]) -> Result<(), PortError> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let required_names: HashSet<&str> = DATADOG_LOGS_AGENT_TOOLS
        .iter()
        .chain(DATADOG_METRICS_AGENT_TOOLS.iter())
        .chain(DATADOG_EVENTS_AGENT_TOOLS.iter())
        .chain(DATADOG_LEAD_AGENT_TOOLS.iter())
        .copied()
        .collect();

    let mut missing_names = required_names
        .into_iter()
        .filter(|name| !available_names.contains(name))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    missing_names.sort();

    if missing_names.is_empty() {
        return Ok(());
    }

    Err(PortError::new(format!(
        "Datadog MCP server is missing required tools: {}",
        missing_names.join(", ")
    )))
}

fn filter_tools(tools: &[Tool], names: &[&str]) -> Vec<Tool> {
    let expected_names: HashSet<&str> = names.iter().copied().collect();

    tools
        .iter()
        .filter(|tool| expected_names.contains(tool.name.as_ref()))
        .cloned()
        .collect()
}

fn build_rig_tools(
    tools: &[Tool],
    names: &[&str],
    client: DatadogMcpHttpClient,
) -> Vec<Box<dyn ToolDyn>> {
    filter_tools(tools, names)
        .into_iter()
        .map(|tool| {
            Box::new(DatadogMcpRigTool {
                definition: tool,
                client: client.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

#[derive(Clone)]
struct DatadogMcpRigTool {
    definition: Tool,
    client: DatadogMcpHttpClient,
}

impl ToolDyn for DatadogMcpRigTool {
    fn name(&self) -> String {
        self.definition.name.to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move {
            ToolDefinition {
                name: self.definition.name.to_string(),
                description: self
                    .definition
                    .description
                    .clone()
                    .unwrap_or_default()
                    .to_string(),
                parameters: serde_json::to_value(&self.definition.input_schema).unwrap_or_default(),
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let name = self.definition.name.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let arguments = serde_json::from_str::<serde_json::Value>(&args)?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    ToolError::ToolCallError(Box::new(io::Error::other(
                        "Datadog MCP tool arguments must be a JSON object",
                    )))
                })?;
            let result = client
                .call_tool(name.to_string(), Some(arguments))
                .await
                .map_err(|transport_error| {
                    let error_message = format!(
                        "Datadog MCP tool {} failed before returning a result: {transport_error}",
                        name
                    );
                    error!(tool_name = %name, error = %transport_error, "{error_message}");
                    ToolError::ToolCallError(Box::new(io::Error::other(error_message)))
                })?;

            if matches!(result.is_error, Some(true)) {
                let error_message = format_datadog_mcp_tool_error(name.as_ref(), &result);
                error!(
                    tool_name = %name,
                    error_message = %error_message,
                    structured_content = ?result.structured_content,
                    content = ?result.content,
                    "Datadog MCP tool returned an error"
                );
                return Err(ToolError::ToolCallError(Box::new(io::Error::other(
                    error_message,
                ))));
            }

            Ok(format_datadog_mcp_tool_success(&result))
        })
    }
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
        StreamableHttpPostResponse::Sse(mut stream, session_id) => {
            while let Some(event) = stream.next().await {
                let event = event.map_err(|error| {
                    PortError::new(format!("Failed to read Datadog MCP SSE event: {error}"))
                })?;
                let payload = event.data.unwrap_or_default();
                if payload.trim().is_empty() {
                    continue;
                }

                let message: ServerJsonRpcMessage =
                    serde_json::from_str(&payload).map_err(|error| {
                        PortError::new(format!(
                            "Failed to deserialize Datadog MCP SSE payload as JSON-RPC: {error}"
                        ))
                    })?;
                match message.into_result() {
                    Some((Ok(result), _)) => return Ok((result, session_id.map(Into::into))),
                    Some((Err(error), _)) => {
                        return Err(PortError::new(format!(
                            "Datadog MCP JSON-RPC error: code={:?} message={} data={}",
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

            Err(PortError::new(
                "Datadog MCP returned an SSE stream without a JSON-RPC response",
            ))
        }
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

fn format_datadog_mcp_tool_success(result: &CallToolResult) -> String {
    let content = render_datadog_mcp_contents(&result.content);
    if !content.is_empty() {
        return content;
    }

    result
        .structured_content
        .as_ref()
        .map_or_else(String::new, serde_json::Value::to_string)
}

fn format_datadog_mcp_tool_error(tool_name: &str, result: &CallToolResult) -> String {
    let mut details = Vec::new();
    let content = render_datadog_mcp_contents(&result.content);
    if !content.is_empty() {
        details.push(format!("content={content}"));
    }

    if let Some(structured_content) = &result.structured_content {
        details.push(format!("structured_content={structured_content}"));
    }

    if let Some(meta) = &result.meta {
        details.push(format!(
            "meta={}",
            serde_json::to_string(meta).unwrap_or_default()
        ));
    }

    if details.is_empty() {
        details.push("no error details returned".to_string());
    }

    format!(
        "Datadog MCP tool {tool_name} returned an error: {}",
        details.join("; ")
    )
}

fn render_datadog_mcp_contents(contents: &[Content]) -> String {
    contents
        .iter()
        .map(render_datadog_mcp_content)
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_datadog_mcp_content(content: &Content) -> String {
    match &content.raw {
        rmcp::model::RawContent::Text(text) => text.text.clone(),
        rmcp::model::RawContent::Resource(resource) => match &resource.resource {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => serde_json::to_string(&content.raw).unwrap_or_default(),
        },
        _ => serde_json::to_string(&content.raw).unwrap_or_default(),
    }
}

fn datadog_mcp_url(site: &str) -> String {
    let site_domain = datadog_site_domain(site);

    format!("https://mcp.{site_domain}/api/unstable/mcp-server/mcp?toolsets={DATADOG_MCP_TOOLSET}")
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
    use reqwest::header::HeaderValue;
    use rmcp::model::Tool;

    use super::{
        DatadogMcpToolConfig, build_datadog_mcp_headers, datadog_mcp_url, datadog_site_domain,
        filter_tools, format_datadog_mcp_tool_error, format_datadog_mcp_tool_success,
        truncate_for_error_message, validate_required_tools,
    };

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    #[test]
    fn builds_datadog_mcp_url_from_site() {
        assert_eq!(
            datadog_mcp_url("datadoghq.eu"),
            "https://mcp.datadoghq.eu/api/unstable/mcp-server/mcp?toolsets=core"
        );
        assert_eq!(
            datadog_mcp_url("ap1.datadoghq.com"),
            "https://mcp.ap1.datadoghq.com/api/unstable/mcp-server/mcp?toolsets=core"
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
    fn validates_required_tool_names() {
        let tools = vec![
            tool("search_datadog_logs"),
            tool("analyze_datadog_logs"),
            tool("search_datadog_metrics"),
            tool("get_datadog_metric"),
            tool("get_datadog_metric_context"),
            tool("search_datadog_events"),
            tool("search_datadog_services"),
            tool("search_datadog_monitors"),
            tool("search_datadog_incidents"),
        ];

        assert!(validate_required_tools(&tools).is_ok());
    }

    #[test]
    fn filters_tools_by_name() {
        let tools = vec![
            tool("search_datadog_logs"),
            tool("search_datadog_metrics"),
            tool("search_datadog_events"),
        ];

        let filtered = filter_tools(&tools, &["search_datadog_logs", "search_datadog_events"]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name.as_ref(), "search_datadog_logs");
        assert_eq!(filtered[1].name.as_ref(), "search_datadog_events");
    }

    #[test]
    fn truncates_error_message_preview() {
        assert_eq!(truncate_for_error_message("abcdef", 4), "abcd...");
        assert_eq!(truncate_for_error_message("abc", 4), "abc");
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
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let result = rmcp::model::CallToolResult {
            content: vec![],
            structured_content: Some(serde_json::json!({"status":"ok"})),
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(
            format_datadog_mcp_tool_success(&result),
            "{\"status\":\"ok\"}"
        );
    }

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text("request failed")],
            structured_content: Some(serde_json::json!({
                "error_code": "FORBIDDEN",
                "details": "permission denied"
            })),
            is_error: Some(true),
            meta: None,
        };

        assert_eq!(
            format_datadog_mcp_tool_error("search_datadog_logs", &result),
            "Datadog MCP tool search_datadog_logs returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }

    #[test]
    fn formats_tool_error_from_embedded_text_resource() {
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::embedded_text(
                "datadog://error",
                "resource failure",
            )],
            structured_content: None,
            is_error: Some(true),
            meta: None,
        };

        assert_eq!(
            format_datadog_mcp_tool_error("search_datadog_metrics", &result),
            "Datadog MCP tool search_datadog_metrics returned an error: content=resource failure"
        );
    }
}
