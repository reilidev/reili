use std::collections::HashSet;
use std::io;

use crate::outbound::agents::connector::ToolCatalogEntry;
pub use crate::outbound::datadog::DatadogMcpToolConfig;
use crate::outbound::datadog::mcp_client::DatadogMcpHttpClient;
use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use tracing::{error, warn};

const DATADOG_SUB_AGENT_TOOLS: &[&str] = &[
    "search_datadog_logs",
    "analyze_datadog_logs",
    "search_datadog_metrics",
    "get_datadog_metric",
    "get_datadog_metric_context",
    "search_datadog_events",
    "search_datadog_dashboards",
    "get_datadog_dashboard",
    "get_synthetics_tests",
    "search_datadog_security_signals",
    "security_findings_schema",
    "search_security_findings",
    "analyze_security_findings",
];
/// One-line catalog summaries for the tools a spawned sub-agent can request, keyed by tool name.
/// Kept short on purpose: the lead only needs enough signal to pick tools; the full schema is
/// injected into the spawned sub-agent.
const DATADOG_SUB_AGENT_TOOL_SUMMARIES: &[(&str, &str)] = &[
    (
        "search_datadog_logs",
        "Search Datadog logs with a query over a time range.",
    ),
    (
        "analyze_datadog_logs",
        "Aggregate Datadog logs to surface patterns, counts, and groupings.",
    ),
    (
        "search_datadog_metrics",
        "Find Datadog metric names matching a search term.",
    ),
    (
        "get_datadog_metric",
        "Query time-series values for a Datadog metric.",
    ),
    (
        "get_datadog_metric_context",
        "Get metadata, tags, and usage context for a Datadog metric.",
    ),
    (
        "search_datadog_events",
        "Search Datadog events (deploys, alerts, changes) over a time range.",
    ),
    (
        "search_datadog_dashboards",
        "Find Datadog dashboards by name or keyword.",
    ),
    (
        "get_datadog_dashboard",
        "Get a Datadog dashboard definition and its widgets.",
    ),
    (
        "get_synthetics_tests",
        "List Datadog Synthetic tests and their status.",
    ),
    (
        "search_datadog_security_signals",
        "Search Datadog security signals over a time range.",
    ),
    (
        "security_findings_schema",
        "Get the schema used to query Datadog security findings.",
    ),
    (
        "search_security_findings",
        "Search Datadog security findings.",
    ),
    (
        "analyze_security_findings",
        "Aggregate Datadog security findings to surface patterns and counts.",
    ),
];
const DATADOG_LEAD_AGENT_TOOLS: &[&str] = &[
    "search_datadog_services",
    "search_datadog_metrics",
    "get_datadog_metric_context",
    "search_datadog_monitors",
    "get_synthetics_tests",
    "search_datadog_security_signals",
];
#[derive(Clone)]
pub struct DatadogMcpToolset {
    tools: Vec<Tool>,
    client: DatadogMcpHttpClient,
}

impl DatadogMcpToolset {
    #[must_use]
    pub fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_tool_adapters(
            &self.tools,
            DATADOG_LEAD_AGENT_TOOLS,
            "lead",
            self.client.clone(),
        )
    }

    #[must_use]
    pub fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_tool_adapters(
            &self.tools,
            DATADOG_SUB_AGENT_TOOLS,
            "sub_agent",
            self.client.clone(),
        )
    }

    /// Catalog entries matching the tools [`Self::sub_agent_tools`] can supply: the allowlisted
    /// tools available on the connected server.
    #[must_use]
    pub fn sub_agent_catalog_entries(&self) -> Vec<ToolCatalogEntry> {
        build_sub_agent_catalog_entries(&self.tools)
    }
}

fn build_sub_agent_catalog_entries(tools: &[Tool]) -> Vec<ToolCatalogEntry> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    DATADOG_SUB_AGENT_TOOL_SUMMARIES
        .iter()
        .filter(|(name, _)| available_names.contains(name))
        .map(|(name, summary)| ToolCatalogEntry::new(name, summary))
        .collect()
}

pub async fn connect_datadog_mcp_toolset(
    config: &DatadogMcpToolConfig,
) -> Result<DatadogMcpToolset, PortError> {
    let (client, tools) = DatadogMcpHttpClient::connect(config).await?;
    Ok(DatadogMcpToolset { tools, client })
}

fn filter_tools(tools: &[Tool], names: &[&str], agent_scope: &str) -> Vec<Tool> {
    let expected_names: HashSet<&str> = names.iter().copied().collect();
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let mut missing_names = expected_names
        .iter()
        .copied()
        .filter(|name| !available_names.contains(name))
        .collect::<Vec<_>>();
    missing_names.sort();

    if !missing_names.is_empty() {
        warn!(
            agent_scope,
            missing_tools = ?missing_names,
            "Datadog MCP server is missing allowlisted tools requested by agent"
        );
    }

    tools
        .iter()
        .filter(|tool| expected_names.contains(tool.name.as_ref()))
        .cloned()
        .collect()
}

fn build_tool_adapters(
    tools: &[Tool],
    names: &[&str],
    agent_scope: &str,
    client: DatadogMcpHttpClient,
) -> Vec<Box<dyn ToolDyn>> {
    filter_tools(tools, names, agent_scope)
        .into_iter()
        .map(|tool| {
            Box::new(DatadogMcpToolAdapter {
                definition: tool,
                client: client.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

#[derive(Clone)]
struct DatadogMcpToolAdapter {
    definition: Tool,
    client: DatadogMcpHttpClient,
}

impl ToolDyn for DatadogMcpToolAdapter {
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

fn render_datadog_mcp_contents(contents: &[ContentBlock]) -> String {
    contents
        .iter()
        .map(render_datadog_mcp_content)
        .filter(|content: &String| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_datadog_mcp_content(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Resource(resource) => match &resource.resource {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => serde_json::to_string(content).unwrap_or_default(),
        },
        _ => serde_json::to_string(content).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::Tool;

    use super::{
        DATADOG_LEAD_AGENT_TOOLS, DATADOG_SUB_AGENT_TOOLS, filter_tools,
        format_datadog_mcp_tool_error, format_datadog_mcp_tool_success,
    };

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    #[test]
    fn catalog_summaries_cover_exactly_the_sub_agent_allowlist() {
        let summary_names: Vec<&str> = super::DATADOG_SUB_AGENT_TOOL_SUMMARIES
            .iter()
            .map(|(name, _)| *name)
            .collect();

        assert_eq!(summary_names, DATADOG_SUB_AGENT_TOOLS);
    }

    #[test]
    fn catalog_entries_include_only_available_tools() {
        let tools = vec![tool("search_datadog_logs"), tool("get_datadog_metric")];

        let names: Vec<String> = super::build_sub_agent_catalog_entries(&tools)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(names, vec!["search_datadog_logs", "get_datadog_metric"]);
    }

    #[test]
    fn filters_tools_by_name() {
        let tools = vec![
            tool("search_datadog_logs"),
            tool("search_datadog_metrics"),
            tool("search_datadog_events"),
        ];

        let filtered = filter_tools(
            &tools,
            &["search_datadog_logs", "search_datadog_events"],
            "sub_agent",
        );

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name.as_ref(), "search_datadog_logs");
        assert_eq!(filtered[1].name.as_ref(), "search_datadog_events");
    }

    #[test]
    fn filters_sub_agent_tools_to_observability_and_security_union() {
        let tools = vec![
            tool("search_datadog_services"),
            tool("search_datadog_logs"),
            tool("analyze_datadog_logs"),
            tool("search_datadog_metrics"),
            tool("get_datadog_metric"),
            tool("get_datadog_metric_context"),
            tool("search_datadog_events"),
            tool("search_datadog_dashboards"),
            tool("get_datadog_dashboard"),
            tool("get_synthetics_tests"),
            tool("search_datadog_security_signals"),
            tool("security_findings_schema"),
            tool("search_security_findings"),
            tool("analyze_security_findings"),
        ];

        let filtered = filter_tools(&tools, DATADOG_SUB_AGENT_TOOLS, "sub_agent");
        let names = filtered
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "search_datadog_logs",
                "analyze_datadog_logs",
                "search_datadog_metrics",
                "get_datadog_metric",
                "get_datadog_metric_context",
                "search_datadog_events",
                "search_datadog_dashboards",
                "get_datadog_dashboard",
                "get_synthetics_tests",
                "search_datadog_security_signals",
                "security_findings_schema",
                "search_security_findings",
                "analyze_security_findings",
            ]
        );
    }

    #[test]
    fn filters_lead_tools_to_triage_reads_and_synthetics() {
        let tools = vec![
            tool("search_datadog_services"),
            tool("search_datadog_metrics"),
            tool("get_datadog_metric_context"),
            tool("search_datadog_monitors"),
            tool("search_datadog_dashboards"),
            tool("get_synthetics_tests"),
            tool("search_datadog_security_signals"),
        ];

        let filtered = filter_tools(&tools, DATADOG_LEAD_AGENT_TOOLS, "lead");
        let names = filtered
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "search_datadog_services",
                "search_datadog_metrics",
                "get_datadog_metric_context",
                "search_datadog_monitors",
                "get_synthetics_tests",
                "search_datadog_security_signals",
            ]
        );
    }

    #[test]
    fn filters_available_subset_without_requiring_full_security_workflow() {
        let tools = vec![
            tool("search_datadog_logs"),
            tool("search_datadog_security_signals"),
            tool("security_findings_schema"),
        ];

        let filtered = filter_tools(&tools, DATADOG_SUB_AGENT_TOOLS, "sub_agent");
        let names = filtered
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "search_datadog_logs",
                "search_datadog_security_signals",
                "security_findings_schema",
            ]
        );
    }

    #[test]
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let mut result = rmcp::model::CallToolResult::success(vec![]);
        result.structured_content = Some(serde_json::json!({"status":"ok"}));

        assert_eq!(
            format_datadog_mcp_tool_success(&result),
            "{\"status\":\"ok\"}"
        );
    }

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let mut result = rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
            "request failed",
        )]);
        result.structured_content = Some(serde_json::json!({
                "error_code": "FORBIDDEN",
                "details": "permission denied"
        }));

        assert_eq!(
            format_datadog_mcp_tool_error("search_datadog_logs", &result),
            "Datadog MCP tool search_datadog_logs returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }

    #[test]
    fn formats_tool_error_from_embedded_text_resource() {
        let result =
            rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::embedded_text(
                "datadog://error",
                "resource failure",
            )]);

        assert_eq!(
            format_datadog_mcp_tool_error("search_datadog_metrics", &result),
            "Datadog MCP tool search_datadog_metrics returned an error: content=resource failure"
        );
    }
}
