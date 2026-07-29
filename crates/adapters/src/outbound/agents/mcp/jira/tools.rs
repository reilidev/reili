use std::collections::HashSet;
use std::sync::Arc;

use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolResult, Tool};
use serde_json::{Map, Value};

use crate::outbound::agents::connector::ToolCatalogEntry;
use crate::outbound::agents::mcp::support;
use crate::outbound::jira::jira_mcp_client::{self, JiraMcpConfig, JiraMcpHttpClient};

const JIRA_MCP_SOURCE_LABEL: &str = "JIRA";

// Confirmed against a live Rovo MCP server's `tools/list` response, `read_jira` and `search_jira`
// permission groups only. Every `write_jira` tool (createJiraIssue, editJiraIssue,
// transitionJiraIssue, addCommentToJiraIssue, addWorklogToJiraIssue) is intentionally excluded,
// matching Reili's read-only investigation principle even if the connected service account
// happens to be granted write access.
const REQUIRED_JIRA_SUB_AGENT_TOOLS: &[&str] = &["searchJiraIssuesUsingJql", "getJiraIssue"];

const JIRA_SUB_AGENT_TOOLS: &[&str] = &[
    "searchJiraIssuesUsingJql",
    "getJiraIssue",
    "getJiraIssueRemoteIssueLinks",
    "getTransitionsForJiraIssue",
];

/// One-line catalog summaries for the tools a spawned sub-agent can request, keyed by tool name.
/// Kept short on purpose: the lead only needs enough signal to pick tools; the full schema is
/// injected into the spawned sub-agent.
const JIRA_SUB_AGENT_TOOL_SUMMARIES: &[(&str, &str)] = &[
    (
        "searchJiraIssuesUsingJql",
        "Search JIRA issues using a JQL query.",
    ),
    (
        "getJiraIssue",
        "Get a JIRA issue's summary, description, status, assignee, comments, and issue links.",
    ),
    (
        "getJiraIssueRemoteIssueLinks",
        "List remote links (e.g. Confluence pages, external URLs) attached to a JIRA issue.",
    ),
    (
        "getTransitionsForJiraIssue",
        "List available workflow transitions and status options for a JIRA issue.",
    ),
];

const CLOUD_ID_ARGUMENT_NAME: &str = "cloudId";

#[derive(Clone)]
pub struct JiraMcpToolset {
    tools: Vec<Tool>,
    client: JiraMcpHttpClient,
    site: Arc<str>,
}

impl JiraMcpToolset {
    #[must_use]
    pub fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_tool_adapters(
            &self.tools,
            JIRA_SUB_AGENT_TOOLS,
            self.client.clone(),
            Arc::clone(&self.site),
        )
    }

    #[must_use]
    pub fn sub_agent_catalog_entries(&self) -> Vec<ToolCatalogEntry> {
        build_sub_agent_catalog_entries(&self.tools)
    }
}

fn build_sub_agent_catalog_entries(tools: &[Tool]) -> Vec<ToolCatalogEntry> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    JIRA_SUB_AGENT_TOOL_SUMMARIES
        .iter()
        .filter(|(name, _)| available_names.contains(name))
        .map(|(name, summary)| ToolCatalogEntry::new(name, summary))
        .collect()
}

pub async fn connect_jira_mcp_toolset(config: &JiraMcpConfig) -> Result<JiraMcpToolset, PortError> {
    let (client, tools) = jira_mcp_client::connect(config).await?;

    validate_required_tools(&tools)?;

    Ok(JiraMcpToolset {
        tools,
        client,
        site: config.site.clone().into(),
    })
}

fn validate_required_tools(tools: &[Tool]) -> Result<(), PortError> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let required_names: HashSet<&str> = REQUIRED_JIRA_SUB_AGENT_TOOLS.iter().copied().collect();

    let mut missing_names = required_names
        .into_iter()
        .filter(|name| !available_names.contains(name))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    missing_names.sort();

    if missing_names.is_empty() {
        return Ok(());
    }

    let mut available_names_sorted: Vec<&str> = available_names.into_iter().collect();
    available_names_sorted.sort_unstable();

    Err(PortError::new(format!(
        "JIRA MCP server is missing required tools: {}. Tools actually returned by tools/list: [{}]",
        missing_names.join(", "),
        available_names_sorted.join(", ")
    )))
}

fn filter_tools(tools: &[Tool], names: &[&str]) -> Vec<Tool> {
    support::filter_tools_by_name(tools, names)
}

fn build_tool_adapters(
    tools: &[Tool],
    names: &[&str],
    client: JiraMcpHttpClient,
    site: Arc<str>,
) -> Vec<Box<dyn ToolDyn>> {
    filter_tools(tools, names)
        .into_iter()
        .map(|tool| {
            Box::new(JiraMcpToolAdapter {
                definition: tool,
                client: client.clone(),
                site: Arc::clone(&site),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

#[derive(Clone)]
struct JiraMcpToolAdapter {
    definition: Tool,
    client: JiraMcpHttpClient,
    site: Arc<str>,
}

impl ToolDyn for JiraMcpToolAdapter {
    fn name(&self) -> String {
        self.definition.name.to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        Box::pin(async move { support::mcp_tool_definition(&self.definition) })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let name = self.definition.name.clone();
        let client = self.client.clone();
        let site = Arc::clone(&self.site);

        Box::pin(async move {
            let mut arguments = support::parse_tool_arguments(JIRA_MCP_SOURCE_LABEL, &args)?;
            // Stamp the configured site onto every call so the LLM cannot target a different
            // Atlassian site than the one this connector is scoped to.
            arguments.insert(
                CLOUD_ID_ARGUMENT_NAME.to_string(),
                Value::String(site.to_string()),
            );

            let result = call_jira_mcp_tool(&client, name.as_ref(), arguments).await?;

            Ok(format_jira_mcp_tool_success(&result))
        })
    }
}

pub(super) async fn call_jira_mcp_tool(
    client: &JiraMcpHttpClient,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<CallToolResult, ToolError> {
    support::call_mcp_tool(JIRA_MCP_SOURCE_LABEL, client, name, arguments).await
}

// About ~5 000 tokens at 4 chars/token; covers issue detail with a long comment thread.
const CONTENT_CHAR_LIMIT: usize = 20_000;

pub(super) fn format_jira_mcp_tool_success(result: &CallToolResult) -> String {
    truncate_if_oversized(support::format_tool_success(result))
}

pub(super) fn truncate_if_oversized(content: String) -> String {
    support::truncate_content(
        content,
        CONTENT_CHAR_LIMIT,
        "narrow the query or request fewer fields to see more",
    )
}

#[cfg(test)]
mod tests {
    use rmcp::model::Tool;
    use serde_json::json;

    use super::{
        CONTENT_CHAR_LIMIT, JIRA_SUB_AGENT_TOOLS, REQUIRED_JIRA_SUB_AGENT_TOOLS, filter_tools,
        format_jira_mcp_tool_success, truncate_if_oversized, validate_required_tools,
    };
    use rmcp::model::{CallToolResult, ContentBlock};

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    #[test]
    fn validates_required_tool_names() {
        let tools = REQUIRED_JIRA_SUB_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
    }

    #[test]
    fn rejects_missing_required_tools() {
        let error =
            validate_required_tools(&[tool("getJiraIssue")]).expect_err("missing tool should fail");

        assert!(error.message.contains("searchJiraIssuesUsingJql"));
    }

    #[test]
    fn missing_required_tools_error_lists_what_the_server_actually_returned() {
        let error =
            validate_required_tools(&[tool("getVisibleJiraProjects"), tool("getJiraIssue")])
                .expect_err("missing tool should fail");

        assert!(
            error
                .message
                .contains("Tools actually returned by tools/list")
        );
        assert!(error.message.contains("getVisibleJiraProjects"));
        assert!(error.message.contains("getJiraIssue"));
    }

    #[test]
    fn filters_tools_by_name() {
        let tools = vec![
            tool("searchJiraIssuesUsingJql"),
            tool("getJiraIssue"),
            tool("createJiraIssue"),
        ];

        let filtered = filter_tools(&tools, JIRA_SUB_AGENT_TOOLS);

        assert_eq!(filtered.len(), 2);
        assert!(
            filtered
                .iter()
                .all(|tool| tool.name.as_ref() != "createJiraIssue")
        );
    }

    #[test]
    fn write_tools_are_never_in_the_sub_agent_allowlist() {
        for write_tool in [
            "createJiraIssue",
            "editJiraIssue",
            "transitionJiraIssue",
            "addCommentToJiraIssue",
            "addWorklogToJiraIssue",
        ] {
            assert!(!JIRA_SUB_AGENT_TOOLS.contains(&write_tool));
        }
    }

    #[test]
    fn does_not_truncate_content_within_limit() {
        let content = "a".repeat(CONTENT_CHAR_LIMIT);
        assert_eq!(truncate_if_oversized(content.clone()), content);
    }

    #[test]
    fn truncates_content_exceeding_limit_and_appends_marker() {
        let line = "x".repeat(100) + "\n";
        let content = line.repeat(CONTENT_CHAR_LIMIT / line.len() + 1);
        let result = truncate_if_oversized(content.clone());
        assert!(result.len() > CONTENT_CHAR_LIMIT);
        assert!(result.contains("[truncated:"));
        assert!(result.len() < content.len());
    }

    #[test]
    fn catalog_summaries_cover_exactly_the_sub_agent_allowlist() {
        let summary_names: Vec<&str> = super::JIRA_SUB_AGENT_TOOL_SUMMARIES
            .iter()
            .map(|(name, _)| *name)
            .collect();

        assert_eq!(summary_names, JIRA_SUB_AGENT_TOOLS);
    }

    #[test]
    fn catalog_entries_include_only_available_tools() {
        let tools = vec![tool("searchJiraIssuesUsingJql"), tool("getJiraIssue")];

        let names: Vec<String> = super::build_sub_agent_catalog_entries(&tools)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "searchJiraIssuesUsingJql".to_string(),
                "getJiraIssue".to_string()
            ]
        );
    }

    #[test]
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let mut result = rmcp::model::CallToolResult::success(vec![]);
        result.structured_content = Some(json!({ "issues": [] }));

        assert_eq!(format_jira_mcp_tool_success(&result), "{\"issues\":[]}");
    }

    #[test]
    fn formats_success_truncates_oversized_content() {
        let oversized = "x".repeat(CONTENT_CHAR_LIMIT * 2);
        let result = CallToolResult::success(vec![ContentBlock::text(oversized.clone())]);

        assert!(format_jira_mcp_tool_success(&result).len() < oversized.len());
    }
}
