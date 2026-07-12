use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::{Map, Value};
use tracing::error;

use crate::outbound::agents::connector::ToolCatalogEntry;
use crate::outbound::jira::jira_mcp_client::{JiraMcpConfig, JiraMcpHttpClient};

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
    let (client, tools) = JiraMcpHttpClient::connect(config).await?;

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
    let expected_names: HashSet<&str> = names.iter().copied().collect();

    tools
        .iter()
        .filter(|tool| expected_names.contains(tool.name.as_ref()))
        .cloned()
        .collect()
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
        let site = Arc::clone(&self.site);

        Box::pin(async move {
            let mut arguments = parse_tool_arguments(&args)?;
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

fn parse_tool_arguments(args: &str) -> Result<Map<String, Value>, ToolError> {
    serde_json::from_str::<Value>(args)?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            ToolError::ToolCallError(Box::new(io::Error::other(
                "JIRA MCP tool arguments must be a JSON object",
            )))
        })
}

pub(super) async fn call_jira_mcp_tool(
    client: &JiraMcpHttpClient,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<CallToolResult, ToolError> {
    let result = client
        .call_tool(name.to_string(), Some(arguments))
        .await
        .map_err(|transport_error| {
            let error_message =
                format!("JIRA MCP tool {name} failed before returning a result: {transport_error}");
            error!(tool_name = %name, error = %transport_error, "{error_message}");
            ToolError::ToolCallError(Box::new(io::Error::other(error_message)))
        })?;

    if matches!(result.is_error, Some(true)) {
        let error_message = format_jira_mcp_tool_error(name, &result);
        error!(
            tool_name = %name,
            error_message = %error_message,
            structured_content = ?result.structured_content,
            content = ?result.content,
            "JIRA MCP tool returned an error"
        );
        return Err(ToolError::ToolCallError(Box::new(io::Error::other(
            error_message,
        ))));
    }

    Ok(result)
}

// About ~5 000 tokens at 4 chars/token; covers issue detail with a long comment thread.
const CONTENT_CHAR_LIMIT: usize = 20_000;

pub(super) fn format_jira_mcp_tool_success(result: &CallToolResult) -> String {
    let content = render_contents(&result.content);
    let content = if !content.is_empty() {
        content
    } else {
        result
            .structured_content
            .as_ref()
            .map_or_else(String::new, serde_json::Value::to_string)
    };

    truncate_if_oversized(content)
}

pub(super) fn truncate_if_oversized(content: String) -> String {
    if content.len() <= CONTENT_CHAR_LIMIT {
        return content;
    }
    let truncated = &content[..CONTENT_CHAR_LIMIT];
    let returned_lines = truncated.lines().count();
    let total_lines = content.lines().count();
    format!(
        "{truncated}\n[truncated: {returned_lines} of {total_lines} lines shown; \
        narrow the query or request fewer fields to see more]"
    )
}

fn format_jira_mcp_tool_error(tool_name: &str, result: &CallToolResult) -> String {
    let mut details = Vec::new();
    let content = render_contents(&result.content);
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
        "JIRA MCP tool {tool_name} returned an error: {}",
        details.join("; ")
    )
}

fn render_contents(contents: &[ContentBlock]) -> String {
    contents
        .iter()
        .map(render_content)
        .filter(|content: &String| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_content(content: &ContentBlock) -> String {
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
    use serde_json::json;

    use super::{
        CONTENT_CHAR_LIMIT, JIRA_SUB_AGENT_TOOLS, REQUIRED_JIRA_SUB_AGENT_TOOLS, filter_tools,
        format_jira_mcp_tool_error, format_jira_mcp_tool_success, truncate_if_oversized,
        validate_required_tools,
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

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let mut result = rmcp::model::CallToolResult::error(vec![rmcp::model::ContentBlock::text(
            "request failed",
        )]);
        result.structured_content =
            Some(json!({ "details": "permission denied", "error_code": "FORBIDDEN" }));

        assert_eq!(
            format_jira_mcp_tool_error("getJiraIssue", &result),
            "JIRA MCP tool getJiraIssue returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }
}
