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

use crate::outbound::github::github_mcp_client::{GitHubMcpConfig, GitHubMcpHttpClient};

const GITHUB_SPECIALIST_AGENT_TOOLS: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
    "get_file_contents",
    "pull_request_read",
];

const SEARCH_QUERY_TOOL_NAMES: &[&str] = &[
    "search_code",
    "search_repositories",
    "search_issues",
    "search_pull_requests",
];
const OWNER_SCOPED_TOOL_NAMES: &[&str] = &["get_file_contents", "pull_request_read"];

#[derive(Clone)]
pub struct GitHubMcpToolset {
    tools: Vec<Tool>,
    client: GitHubMcpHttpClient,
    scope_policy: GithubScopePolicy,
}

impl GitHubMcpToolset {
    #[must_use]
    pub fn specialist_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        build_tool_adapters(
            &self.tools,
            GITHUB_SPECIALIST_AGENT_TOOLS,
            self.client.clone(),
            self.scope_policy.clone(),
        )
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
    let required_names: HashSet<&str> = GITHUB_SPECIALIST_AGENT_TOOLS.iter().copied().collect();

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
            let arguments = serde_json::from_str::<Value>(&args)?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    ToolError::ToolCallError(Box::new(io::Error::other(
                        "GitHub MCP tool arguments must be a JSON object",
                    )))
                })?;
            validate_scope(&name, &arguments, &scope_policy).map_err(|error| {
                ToolError::ToolCallError(Box::new(io::Error::other(error.message)))
            })?;

            let result = client
                .call_tool(name.to_string(), Some(arguments))
                .await
                .map_err(|transport_error| {
                    let error_message = format!(
                        "GitHub MCP tool {} failed before returning a result: {transport_error}",
                        name
                    );
                    error!(tool_name = %name, error = %transport_error, "{error_message}");
                    ToolError::ToolCallError(Box::new(io::Error::other(error_message)))
                })?;

            if matches!(result.is_error, Some(true)) {
                let error_message = format_github_mcp_tool_error(name.as_ref(), &result);
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

            Ok(format_github_mcp_tool_success(&result))
        })
    }
}

fn validate_scope(
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

fn format_github_mcp_tool_success(result: &CallToolResult) -> String {
    let content = render_contents(&result.content);
    if !content.is_empty() {
        return content;
    }

    result
        .structured_content
        .as_ref()
        .map_or_else(String::new, serde_json::Value::to_string)
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
        GITHUB_SPECIALIST_AGENT_TOOLS, filter_tools, format_github_mcp_tool_error,
        format_github_mcp_tool_success, validate_required_tools, validate_scope,
    };
    use reili_core::source_code::github::GithubScopePolicy;

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    fn scope_policy() -> GithubScopePolicy {
        GithubScopePolicy::new("acme".to_string()).expect("create scope policy")
    }

    #[test]
    fn validates_required_tool_names() {
        let tools = GITHUB_SPECIALIST_AGENT_TOOLS
            .iter()
            .map(|name| tool(name))
            .collect::<Vec<_>>();

        assert!(validate_required_tools(&tools).is_ok());
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
            "get_file_contents",
            json!({ "owner": "other", "repo": "svc" })
                .as_object()
                .expect("object"),
            &scope_policy(),
        )
        .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[test]
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let result = rmcp::model::CallToolResult {
            content: vec![],
            structured_content: Some(json!({ "items": [] })),
            is_error: Some(false),
            meta: None,
        };

        assert_eq!(format_github_mcp_tool_success(&result), "{\"items\":[]}");
    }

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let result = rmcp::model::CallToolResult {
            content: vec![rmcp::model::Content::text("request failed")],
            structured_content: Some(
                json!({ "details": "permission denied", "error_code": "FORBIDDEN" }),
            ),
            is_error: Some(true),
            meta: None,
        };

        assert_eq!(
            format_github_mcp_tool_error("search_code", &result),
            "GitHub MCP tool search_code returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }
}
