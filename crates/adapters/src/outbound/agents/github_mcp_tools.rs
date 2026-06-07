use std::collections::HashSet;
use std::io;

use reili_core::error::PortError;
use reili_core::source_code::github::GithubScopePolicy;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolResult, Content, Tool};
use serde_json::{Map, Value};
use tracing::error;

use super::github_read_file_tool::GitHubReadFileToolAdapter;
use crate::outbound::github::github_mcp_client::{GitHubMcpConfig, GitHubMcpHttpClient};

const REQUIRED_GITHUB_SPECIALIST_AGENT_TOOLS: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
    "get_file_contents",
    "pull_request_read",
];

#[cfg(test)]
const OPTIONAL_GITHUB_SPECIALIST_AGENT_TOOLS: &[&str] = &[
    "actions_get",
    "actions_list",
    "get_job_logs",
    "get_dependabot_alert",
    "list_dependabot_alerts",
];

// `get_file_contents` is intentionally absent: file reads are exposed to the agent through the
// `read_file` wrapper (see `GitHubReadFileToolAdapter`), which forwards to the server-side
// `get_file_contents` tool but returns a bounded, line-numbered window.
const GITHUB_SPECIALIST_AGENT_TOOLS: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
    "pull_request_read",
    "actions_get",
    "actions_list",
    "get_job_logs",
    "get_dependabot_alert",
    "list_dependabot_alerts",
];

/// Agent-facing name of the windowed file read wrapper.
pub(super) const READ_FILE_TOOL_NAME: &str = "read_file";

/// Server-side tool the `read_file` wrapper forwards to.
pub(super) const GET_FILE_CONTENTS_TOOL_NAME: &str = "get_file_contents";

const SEARCH_QUERY_TOOL_NAMES: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
];
const OWNER_SCOPED_TOOL_NAMES: &[&str] = &[
    READ_FILE_TOOL_NAME,
    "pull_request_read",
    "actions_get",
    "actions_list",
    "get_job_logs",
    "get_dependabot_alert",
    "list_dependabot_alerts",
];

#[derive(Clone)]
pub struct GitHubMcpToolset {
    tools: Vec<Tool>,
    client: GitHubMcpHttpClient,
    scope_policy: GithubScopePolicy,
}

impl GitHubMcpToolset {
    #[must_use]
    pub fn specialist_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        let mut adapters = build_tool_adapters(
            &self.tools,
            GITHUB_SPECIALIST_AGENT_TOOLS,
            self.client.clone(),
            self.scope_policy.clone(),
        );
        adapters.push(Box::new(GitHubReadFileToolAdapter::new(
            self.client.clone(),
            self.scope_policy.clone(),
        )) as Box<dyn ToolDyn>);
        adapters
    }
}

pub async fn connect_github_mcp_toolset(
    config: &GitHubMcpConfig,
    github_scope_org: String,
) -> Result<GitHubMcpToolset, PortError> {
    let (client, tools) = GitHubMcpHttpClient::connect(config).await?;
    let scope_policy = GithubScopePolicy::new(github_scope_org)?;

    validate_required_tools(&tools)?;

    Ok(GitHubMcpToolset {
        tools,
        client,
        scope_policy,
    })
}

fn validate_required_tools(tools: &[Tool]) -> Result<(), PortError> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let required_names: HashSet<&str> = REQUIRED_GITHUB_SPECIALIST_AGENT_TOOLS
        .iter()
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
        "GitHub MCP server is missing required tools: {}",
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

fn build_tool_adapters(
    tools: &[Tool],
    names: &[&str],
    client: GitHubMcpHttpClient,
    scope_policy: GithubScopePolicy,
) -> Vec<Box<dyn ToolDyn>> {
    filter_tools(tools, names)
        .into_iter()
        .map(|tool| {
            Box::new(GitHubMcpToolAdapter {
                definition: tool,
                client: client.clone(),
                scope_policy: scope_policy.clone(),
            }) as Box<dyn ToolDyn>
        })
        .collect()
}

#[derive(Clone)]
struct GitHubMcpToolAdapter {
    definition: Tool,
    client: GitHubMcpHttpClient,
    scope_policy: GithubScopePolicy,
}

impl ToolDyn for GitHubMcpToolAdapter {
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
        let scope_policy = self.scope_policy.clone();

        Box::pin(async move {
            let arguments = parse_tool_arguments(&args)?;
            validate_scope(&name, &arguments, &scope_policy).map_err(|error| {
                ToolError::ToolCallError(Box::new(io::Error::other(error.message)))
            })?;

            let result = call_github_mcp_tool(&client, name.as_ref(), arguments).await?;

            Ok(format_github_mcp_tool_success(&result))
        })
    }
}

fn parse_tool_arguments(args: &str) -> Result<Map<String, Value>, ToolError> {
    serde_json::from_str::<Value>(args)?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            ToolError::ToolCallError(Box::new(io::Error::other(
                "GitHub MCP tool arguments must be a JSON object",
            )))
        })
}

pub(super) async fn call_github_mcp_tool(
    client: &GitHubMcpHttpClient,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<CallToolResult, ToolError> {
    let result = client
        .call_tool(name.to_string(), Some(arguments))
        .await
        .map_err(|transport_error| {
            let error_message = format!(
                "GitHub MCP tool {name} failed before returning a result: {transport_error}"
            );
            error!(tool_name = %name, error = %transport_error, "{error_message}");
            ToolError::ToolCallError(Box::new(io::Error::other(error_message)))
        })?;

    if matches!(result.is_error, Some(true)) {
        let error_message = format_github_mcp_tool_error(name, &result);
        error!(
            tool_name = %name,
            error_message = %error_message,
            structured_content = ?result.structured_content,
            content = ?result.content,
            "GitHub MCP tool returned an error"
        );
        return Err(ToolError::ToolCallError(Box::new(io::Error::other(
            error_message,
        ))));
    }

    Ok(result)
}

pub(super) fn validate_scope(
    tool_name: &str,
    arguments: &Map<String, Value>,
    scope_policy: &GithubScopePolicy,
) -> Result<(), PortError> {
    if SEARCH_QUERY_TOOL_NAMES.contains(&tool_name) {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| PortError::invalid_input(format!("{tool_name} requires query")))?;
        scope_policy.validate_query(query)?;
    }

    if OWNER_SCOPED_TOOL_NAMES.contains(&tool_name) {
        let owner = arguments
            .get("owner")
            .and_then(Value::as_str)
            .ok_or_else(|| PortError::invalid_input(format!("{tool_name} requires owner")))?;
        scope_policy.validate_owner(owner)?;
    }

    Ok(())
}

// About ~5 000 tokens at 4 chars/token; covers file content and large directory listings.
const FILE_CONTENT_CHAR_LIMIT: usize = 20_000;

pub(super) fn format_github_mcp_tool_success(result: &CallToolResult) -> String {
    let content = render_contents(&result.content);
    if !content.is_empty() {
        return content;
    }

    result
        .structured_content
        .as_ref()
        .map_or_else(String::new, serde_json::Value::to_string)
}

pub(super) fn truncate_if_oversized(content: String) -> String {
    if content.len() <= FILE_CONTENT_CHAR_LIMIT {
        return content;
    }
    let truncated = &content[..FILE_CONTENT_CHAR_LIMIT];
    let returned_lines = truncated.lines().count();
    let total_lines = content.lines().count();
    format!(
        "{truncated}\n[truncated: {returned_lines} of {total_lines} lines shown; \
        request a specific line range to read more]"
    )
}

fn format_github_mcp_tool_error(tool_name: &str, result: &CallToolResult) -> String {
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
        "GitHub MCP tool {tool_name} returned an error: {}",
        details.join("; ")
    )
}

fn render_contents(contents: &[Content]) -> String {
    contents
        .iter()
        .map(render_content)
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_content(content: &Content) -> String {
    match &content.raw {
        rmcp::model::RawContent::Text(text) => text.text.clone(),
        rmcp::model::RawContent::Resource(resource) => match &resource.resource {
            rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
            _ => serde_json::to_string(&content.raw).unwrap_or_default(),
        },
        _ => serde_json::to_string(&content.raw).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::Tool;
    use serde_json::json;

    use super::{
        FILE_CONTENT_CHAR_LIMIT, GITHUB_SPECIALIST_AGENT_TOOLS,
        OPTIONAL_GITHUB_SPECIALIST_AGENT_TOOLS, REQUIRED_GITHUB_SPECIALIST_AGENT_TOOLS,
        filter_tools, format_github_mcp_tool_error, format_github_mcp_tool_success,
        truncate_if_oversized, validate_required_tools, validate_scope,
    };
    use reili_core::source_code::github::GithubScopePolicy;
    use rmcp::model::{CallToolResult, Content};

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    fn scope_policy() -> GithubScopePolicy {
        GithubScopePolicy::new("acme".to_string()).expect("create scope policy")
    }

    #[test]
    fn validates_required_tool_names() {
        let tools = REQUIRED_GITHUB_SPECIALIST_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
    }

    #[test]
    fn allows_missing_optional_tool_names() {
        let tools = REQUIRED_GITHUB_SPECIALIST_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
        assert!(!OPTIONAL_GITHUB_SPECIALIST_AGENT_TOOLS.is_empty());
    }

    #[test]
    fn filters_tools_by_name() {
        let tools = vec![
            tool("search_code"),
            tool("search_repositories"),
            tool("search_pull_requests"),
        ];

        let filtered = filter_tools(&tools, &["search_code", "search_pull_requests"]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name.as_ref(), "search_code");
        assert_eq!(filtered[1].name.as_ref(), "search_pull_requests");
    }

    #[test]
    fn validates_org_scope_for_search_tools() {
        let result = validate_scope(
            "search_code",
            json!({ "query": "org:acme language:rust" })
                .as_object()
                .expect("object"),
            &scope_policy(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn validates_owner_scope_for_owner_tools() {
        let error = validate_scope(
            "pull_request_read",
            json!({ "owner": "other", "repo": "svc" })
                .as_object()
                .expect("object"),
            &scope_policy(),
        )
        .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[test]
    fn validates_owner_scope_for_dependabot_tools() {
        let error = validate_scope(
            "get_dependabot_alert",
            json!({ "owner": "other", "repo": "svc", "alertNumber": 1 })
                .as_object()
                .expect("object"),
            &scope_policy(),
        )
        .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[test]
    fn does_not_truncate_content_within_limit() {
        let content = "a".repeat(FILE_CONTENT_CHAR_LIMIT);
        assert_eq!(truncate_if_oversized(content.clone()), content);
    }

    #[test]
    fn truncates_content_exceeding_limit_and_appends_marker() {
        let line = "x".repeat(100) + "\n";
        let content = line.repeat(FILE_CONTENT_CHAR_LIMIT / line.len() + 1);
        let result = truncate_if_oversized(content.clone());
        assert!(result.len() > FILE_CONTENT_CHAR_LIMIT);
        assert!(result.contains("[truncated:"));
        assert!(result.contains("of"));
        assert!(result.contains("lines shown"));
        assert!(result.len() < content.len());
    }

    #[test]
    fn get_file_contents_is_not_exposed_to_agent() {
        assert!(!GITHUB_SPECIALIST_AGENT_TOOLS.contains(&"get_file_contents"));
    }

    #[test]
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let mut result = rmcp::model::CallToolResult::success(vec![]);
        result.structured_content = Some(json!({ "items": [] }));

        assert_eq!(format_github_mcp_tool_success(&result), "{\"items\":[]}");
    }

    #[test]
    fn formats_success_does_not_truncate_oversized_passthrough_content() {
        let oversized = "x".repeat(FILE_CONTENT_CHAR_LIMIT + 1);
        let result = CallToolResult::success(vec![Content::text(oversized.clone())]);

        assert_eq!(format_github_mcp_tool_success(&result), oversized);
    }

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let mut result =
            rmcp::model::CallToolResult::error(vec![rmcp::model::Content::text("request failed")]);
        result.structured_content =
            Some(json!({ "details": "permission denied", "error_code": "FORBIDDEN" }));

        assert_eq!(
            format_github_mcp_tool_error("search_code", &result),
            "GitHub MCP tool search_code returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }
}
