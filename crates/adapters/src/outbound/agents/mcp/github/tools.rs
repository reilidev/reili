use std::collections::HashSet;

use reili_core::error::PortError;
use reili_core::source_code::github::GithubScopePolicy;
use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolResult, Tool};
use serde_json::{Map, Value};

use super::read_file::GitHubReadFileToolAdapter;
use crate::outbound::agents::connector::ToolCatalogEntry;
use crate::outbound::agents::mcp::support;
use crate::outbound::github::github_mcp_client::{self, GitHubMcpConfig, GitHubMcpHttpClient};

const GITHUB_MCP_SOURCE_LABEL: &str = "GitHub";

const REQUIRED_GITHUB_SUB_AGENT_TOOLS: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
    "get_file_contents",
    "pull_request_read",
];

#[cfg(test)]
const OPTIONAL_GITHUB_SUB_AGENT_TOOLS: &[&str] = &[
    "actions_get",
    "actions_list",
    "get_job_logs",
    "get_dependabot_alert",
    "list_dependabot_alerts",
];

// `get_file_contents` is intentionally absent: file reads are exposed to the agent through the
// `read_file` wrapper (see `GitHubReadFileToolAdapter`), which forwards to the server-side
// `get_file_contents` tool but returns a bounded, line-numbered window.
const GITHUB_SUB_AGENT_TOOLS: &[&str] = &[
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

/// One-line catalog summaries for the tools a spawned sub-agent can request, keyed by tool name.
/// Kept short on purpose: the lead only needs enough signal to pick tools; the full schema is
/// injected into the spawned sub-agent.
const GITHUB_SUB_AGENT_TOOL_SUMMARIES: &[(&str, &str)] = &[
    (
        "search_code",
        "Search code across the GitHub org using GitHub code search syntax.",
    ),
    (
        "search_repositories",
        "Find repositories in the org by name, topic, or description.",
    ),
    (
        "search_issues",
        "Search issues in the org by keywords, labels, author, and state.",
    ),
    (
        "search_pull_requests",
        "Search pull requests in the org by keywords, author, state, and dates.",
    ),
    (
        "pull_request_read",
        "Read one pull request: metadata, diff, files, reviews, and comments.",
    ),
    (
        "actions_get",
        "Get details of a GitHub Actions workflow run or job.",
    ),
    (
        "actions_list",
        "List GitHub Actions workflows, runs, or jobs for a repository.",
    ),
    (
        "get_job_logs",
        "Fetch logs for a GitHub Actions job, including failed jobs.",
    ),
    (
        "get_dependabot_alert",
        "Get one Dependabot alert by number for a repository.",
    ),
    (
        "list_dependabot_alerts",
        "List Dependabot alerts for a repository.",
    ),
];

const READ_FILE_TOOL_SUMMARY: &str =
    "Read a repository file as a bounded, line-numbered window (supports offset/limit).";

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
    pub fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        let mut adapters = build_tool_adapters(
            &self.tools,
            GITHUB_SUB_AGENT_TOOLS,
            self.client.clone(),
            self.scope_policy.clone(),
        );
        adapters.push(Box::new(GitHubReadFileToolAdapter::new(
            self.client.clone(),
            self.scope_policy.clone(),
        )) as Box<dyn ToolDyn>);
        adapters
    }

    /// Catalog entries matching the tools [`Self::sub_agent_tools`] can supply: the allowlisted
    /// tools available on the connected server, plus the always-present `read_file` wrapper.
    #[must_use]
    pub fn sub_agent_catalog_entries(&self) -> Vec<ToolCatalogEntry> {
        build_sub_agent_catalog_entries(&self.tools)
    }
}

fn build_sub_agent_catalog_entries(tools: &[Tool]) -> Vec<ToolCatalogEntry> {
    let available_names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    let mut entries: Vec<ToolCatalogEntry> = GITHUB_SUB_AGENT_TOOL_SUMMARIES
        .iter()
        .filter(|(name, _)| available_names.contains(name))
        .map(|(name, summary)| ToolCatalogEntry::new(name, summary))
        .collect();
    entries.push(ToolCatalogEntry::new(
        READ_FILE_TOOL_NAME,
        READ_FILE_TOOL_SUMMARY,
    ));
    entries
}

pub async fn connect_github_mcp_toolset(
    config: &GitHubMcpConfig,
    github_scope_org: String,
) -> Result<GitHubMcpToolset, PortError> {
    let (client, tools) = github_mcp_client::connect(config).await?;
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
    let required_names: HashSet<&str> = REQUIRED_GITHUB_SUB_AGENT_TOOLS.iter().copied().collect();

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
    support::filter_tools_by_name(tools, names)
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
        Box::pin(async move { support::mcp_tool_definition(&self.definition) })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let name = self.definition.name.clone();
        let client = self.client.clone();
        let scope_policy = self.scope_policy.clone();

        Box::pin(async move {
            let arguments = support::parse_tool_arguments(GITHUB_MCP_SOURCE_LABEL, &args)?;
            validate_scope(&name, &arguments, &scope_policy).map_err(|error| {
                ToolError::ToolCallError(Box::new(std::io::Error::other(error.message)))
            })?;

            let result = call_github_mcp_tool(&client, name.as_ref(), arguments).await?;

            Ok(support::format_tool_success(&result))
        })
    }
}

pub(super) async fn call_github_mcp_tool(
    client: &GitHubMcpHttpClient,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<CallToolResult, ToolError> {
    support::call_mcp_tool(GITHUB_MCP_SOURCE_LABEL, client, name, arguments).await
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
    support::format_tool_success(result)
}

pub(super) fn truncate_if_oversized(content: String) -> String {
    support::truncate_content(
        content,
        FILE_CONTENT_CHAR_LIMIT,
        "request a specific line range to read more",
    )
}

#[cfg(test)]
mod tests {
    use rmcp::model::Tool;
    use serde_json::json;

    use super::{
        FILE_CONTENT_CHAR_LIMIT, GITHUB_SUB_AGENT_TOOLS, OPTIONAL_GITHUB_SUB_AGENT_TOOLS,
        REQUIRED_GITHUB_SUB_AGENT_TOOLS, filter_tools, format_github_mcp_tool_success,
        truncate_if_oversized, validate_required_tools, validate_scope,
    };
    use reili_core::source_code::github::GithubScopePolicy;
    use rmcp::model::{CallToolResult, ContentBlock};

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    fn scope_policy() -> GithubScopePolicy {
        GithubScopePolicy::new("acme".to_string()).expect("create scope policy")
    }

    #[test]
    fn validates_required_tool_names() {
        let tools = REQUIRED_GITHUB_SUB_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
    }

    #[test]
    fn allows_missing_optional_tool_names() {
        let tools = REQUIRED_GITHUB_SUB_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
        assert!(!OPTIONAL_GITHUB_SUB_AGENT_TOOLS.is_empty());
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
        assert!(!GITHUB_SUB_AGENT_TOOLS.contains(&"get_file_contents"));
    }

    #[test]
    fn catalog_summaries_cover_exactly_the_sub_agent_allowlist() {
        let summary_names: Vec<&str> = super::GITHUB_SUB_AGENT_TOOL_SUMMARIES
            .iter()
            .map(|(name, _)| *name)
            .collect();

        assert_eq!(summary_names, GITHUB_SUB_AGENT_TOOLS);
    }

    #[test]
    fn catalog_entries_include_only_available_tools_plus_read_file() {
        let tools = vec![tool("search_code"), tool("pull_request_read")];

        let names: Vec<String> = super::build_sub_agent_catalog_entries(&tools)
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        assert_eq!(names, vec!["search_code", "pull_request_read", "read_file"]);
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
        let result = CallToolResult::success(vec![ContentBlock::text(oversized.clone())]);

        assert_eq!(format_github_mcp_tool_success(&result), oversized);
    }
}
