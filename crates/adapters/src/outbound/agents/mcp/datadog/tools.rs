use std::collections::HashSet;

use crate::outbound::agents::connector::ToolCatalogEntry;
use crate::outbound::agents::mcp::support;
pub use crate::outbound::datadog::DatadogMcpToolConfig;
use crate::outbound::datadog::mcp_client::{self, DatadogMcpHttpClient};
use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::Tool;
use tracing::warn;

const DATADOG_MCP_SOURCE_LABEL: &str = "Datadog";

const DATADOG_SUB_AGENT_TOOLS: &[&str] = &[
    "search_datadog_services",
    "search_datadog_logs",
    "analyze_datadog_logs",
    "search_datadog_metrics",
    "get_datadog_metric",
    "get_datadog_metric_context",
    "search_datadog_events",
    "search_datadog_monitors",
    "search_datadog_dashboards",
    "get_datadog_dashboard",
    "get_synthetics_tests",
    "search_datadog_security_signals",
    "search_datadog_security_findings",
    "analyze_security_findings",
];
/// One-line catalog summaries for the tools a spawned sub-agent can request, keyed by tool name.
/// Kept short on purpose: the lead only needs enough signal to pick tools; the full schema is
/// injected into the spawned sub-agent.
const DATADOG_SUB_AGENT_TOOL_SUMMARIES: &[(&str, &str)] = &[
    (
        "search_datadog_services",
        "Find Datadog services by name or keyword.",
    ),
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
        "search_datadog_monitors",
        "Find Datadog monitors by name, tag, or status.",
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
        "search_datadog_security_findings",
        "Search Datadog security findings.",
    ),
    (
        "analyze_security_findings",
        "Aggregate Datadog security findings to surface patterns and counts.",
    ),
];
#[derive(Clone)]
pub struct DatadogMcpToolset {
    tools: Vec<Tool>,
    client: DatadogMcpHttpClient,
}

impl DatadogMcpToolset {
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
    let (client, tools) = mcp_client::connect(config).await?;
    Ok(DatadogMcpToolset { tools, client })
}

fn filter_tools(tools: &[Tool], names: &[&str], agent_scope: &str) -> Vec<Tool> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let mut missing_names = names
        .iter()
        .copied()
        .filter(|name| !available_names.contains(name))
        .collect::<Vec<_>>();
    missing_names.sort_unstable();

    if !missing_names.is_empty() {
        warn!(
            agent_scope,
            missing_tools = ?missing_names,
            "Datadog MCP server is missing allowlisted tools requested by agent"
        );
    }

    support::filter_tools_by_name(tools, names)
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
        Box::pin(async move { support::mcp_tool_definition(&self.definition) })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let name = self.definition.name.clone();
        let client = self.client.clone();

        Box::pin(async move {
            let arguments = support::parse_tool_arguments(DATADOG_MCP_SOURCE_LABEL, &args)?;
            let result =
                support::call_mcp_tool(DATADOG_MCP_SOURCE_LABEL, &client, name.as_ref(), arguments)
                    .await?;

            Ok(support::format_tool_success(&result))
        })
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::Tool;

    use super::{DATADOG_SUB_AGENT_TOOLS, filter_tools};

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
    fn catalog_entries_include_lead_tools_moved_to_sub_agent_scope() {
        let tools = vec![
            tool("search_datadog_services"),
            tool("search_datadog_monitors"),
        ];

        let names: Vec<String> = super::build_sub_agent_catalog_entries(&tools)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(
            names,
            vec!["search_datadog_services", "search_datadog_monitors"]
        );
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
            tool("search_datadog_monitors"),
            tool("search_datadog_dashboards"),
            tool("get_datadog_dashboard"),
            tool("get_synthetics_tests"),
            tool("search_datadog_security_signals"),
            tool("search_datadog_security_findings"),
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
                "search_datadog_services",
                "search_datadog_logs",
                "analyze_datadog_logs",
                "search_datadog_metrics",
                "get_datadog_metric",
                "get_datadog_metric_context",
                "search_datadog_events",
                "search_datadog_monitors",
                "search_datadog_dashboards",
                "get_datadog_dashboard",
                "get_synthetics_tests",
                "search_datadog_security_signals",
                "search_datadog_security_findings",
                "analyze_security_findings",
            ]
        );
    }

    #[test]
    fn filters_available_subset_without_requiring_full_security_workflow() {
        let tools = vec![
            tool("search_datadog_logs"),
            tool("search_datadog_security_signals"),
            tool("search_datadog_security_findings"),
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
                "search_datadog_security_findings",
            ]
        );
    }
}
