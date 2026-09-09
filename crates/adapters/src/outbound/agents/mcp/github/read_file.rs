use reili_core::source_code::github::GithubScopePolicy;
use rig::completion::ToolDefinition;
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use rmcp::model::CallToolResult;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::tools::{
    GET_FILE_CONTENTS_TOOL_NAME, READ_FILE_TOOL_NAME, call_github_mcp_tool,
    format_github_mcp_tool_success, truncate_if_oversized, validate_scope,
};
use crate::outbound::github::github_mcp_client::GitHubMcpHttpClient;

/// Default number of lines returned by `read_file` when the caller omits `limit`.
const DEFAULT_READ_FILE_LINE_LIMIT: usize = 400;

/// Builds the `read_file` wrapper around the server-side `get_file_contents` tool.
///
/// The MCP server still fetches the whole file, but only a bounded, line-numbered window enters the
/// LLM context. The agent can iterate (adjust `offset`/`limit`) instead of loading large files
/// wholesale. Directory paths and other non-file responses are passed through unchanged.
pub(super) fn github_read_file_tool(
    client: GitHubMcpHttpClient,
    scope_policy: GithubScopePolicy,
) -> DynamicTool {
    let definition = read_file_tool_definition();

    DynamicTool::new(
        definition.name,
        definition.description,
        definition.parameters,
        move |_context, arguments_value| {
            let client = client.clone();
            let scope_policy = scope_policy.clone();

            Box::pin(async move {
                let arguments: ReadFileArguments = serde_json::from_value(arguments_value)
                    .map_err(|error| {
                        ToolExecutionError::invalid_args(format!(
                            "read_file arguments were invalid: {error}"
                        ))
                    })?;
                let forwarded = arguments.to_forwarded_arguments()?;

                validate_scope(READ_FILE_TOOL_NAME, &forwarded, &scope_policy)
                    .map_err(|error| ToolExecutionError::permission_denied(error.message))?;

                let result =
                    call_github_mcp_tool(&client, GET_FILE_CONTENTS_TOOL_NAME, forwarded).await?;

                let text = match extract_file_text(&result) {
                    Some(file_text) => arguments.line_window().apply(&file_text),
                    // Directory listings, resource links (>= 1MB files), and other non-file
                    // responses have no meaningful line window; cap them with the char limit
                    // instead.
                    None => truncate_if_oversized(format_github_mcp_tool_success(&result)),
                };
                Ok(ToolOutput::text(text))
            })
        },
    )
}

fn read_file_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: READ_FILE_TOOL_NAME.to_string(),
        description:
            "Read one file from a GitHub repository as a line-numbered window. Returns at \
            most `limit` lines starting at `offset` (1-based) so large files can be read \
            incrementally instead of loading the whole file into context. Locate the region first \
            (for example with `search_code`), then read that range; widen `offset`/`limit` to see \
            more. A directory path returns its entry listing instead of file content."
                .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "owner": {
                    "type": "string",
                    "description": "Repository owner. Must be the configured scope org."
                },
                "repo": {
                    "type": "string",
                    "description": "Repository name."
                },
                "path": {
                    "type": "string",
                    "description": "Path to the file within the repository. A directory path returns its entry listing."
                },
                "ref": {
                    "type": "string",
                    "description": "Optional git ref (branch, tag, or commit SHA). Defaults to the repository default branch."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line number to start reading from. Defaults to 1."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum number of lines to return. Defaults to 400."
                }
            },
            "required": ["owner", "repo", "path"]
        }),
    }
}

/// Deserialized `read_file` arguments.
///
/// `offset`/`limit` are wrapper-only parameters: they shape the returned window but are never
/// forwarded to the server-side `get_file_contents` tool, which does not understand them.
#[derive(Debug, Deserialize)]
struct ReadFileArguments {
    owner: String,
    repo: String,
    path: String,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments forwarded to the server-side `get_file_contents` tool (window params excluded).
#[derive(Debug, Serialize)]
struct GetFileContentsArguments<'a> {
    owner: &'a str,
    repo: &'a str,
    path: &'a str,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    git_ref: Option<&'a str>,
}

impl ReadFileArguments {
    fn line_window(&self) -> LineWindow {
        LineWindow::new(self.offset, self.limit)
    }

    fn to_forwarded_arguments(&self) -> Result<Map<String, Value>, ToolExecutionError> {
        let value = serde_json::to_value(GetFileContentsArguments {
            owner: &self.owner,
            repo: &self.repo,
            path: &self.path,
            git_ref: self.git_ref.as_deref(),
        })
        .map_err(|error| {
            ToolExecutionError::other(format!(
                "failed to serialize read_file forwarded arguments: {error}"
            ))
        })?;
        match value {
            Value::Object(map) => Ok(map),
            _ => Err(ToolExecutionError::other(
                "read_file forwarded arguments must serialize to a JSON object",
            )),
        }
    }
}

#[derive(Clone, Copy)]
struct LineWindow {
    offset: usize,
    limit: usize,
}

impl LineWindow {
    /// Resolves the window, applying defaults and treating non-positive values as absent.
    fn new(offset: Option<usize>, limit: Option<usize>) -> Self {
        let offset = offset.filter(|value| *value > 0).unwrap_or(1);
        let limit = limit
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_READ_FILE_LINE_LIMIT);
        Self { offset, limit }
    }

    fn apply(self, content: &str) -> String {
        let total_lines = content.lines().count();
        let start_index = self.offset - 1;

        if start_index >= total_lines {
            return format!(
                "[no content: requested offset {} but the file has {total_lines} line(s)]",
                self.offset
            );
        }

        let numbered = content
            .lines()
            .enumerate()
            .skip(start_index)
            .take(self.limit)
            .map(|(index, line)| format!("{}\t{line}", index + 1))
            .collect::<Vec<_>>();
        let last_line = start_index + numbered.len();
        let body = numbered.join("\n");

        let is_partial = start_index > 0 || last_line < total_lines;
        let windowed = if is_partial {
            format!(
                "{body}\n[showing lines {}-{last_line} of {total_lines}; \
                pass offset/limit to read a different range]",
                self.offset
            )
        } else {
            body
        };

        truncate_if_oversized(windowed)
    }
}

/// Extracts file content from a `get_file_contents` result.
///
/// The GitHub MCP server delivers file bodies as embedded text resources, while directory listings
/// and `ResourceLink` responses (>= 1MB files) arrive as other content shapes. Returning `None`
/// signals "not a readable text file" so the caller can fall back to passthrough formatting.
fn extract_file_text(result: &CallToolResult) -> Option<String> {
    let texts = result
        .content
        .iter()
        .filter_map(|content| match content {
            rmcp::model::ContentBlock::Resource(resource) => match &resource.resource {
                rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
                    Some(text.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::{CallToolResult, ContentBlock};
    use serde_json::{Value, json};

    use super::{
        DEFAULT_READ_FILE_LINE_LIMIT, LineWindow, READ_FILE_TOOL_NAME, ReadFileArguments,
        extract_file_text, read_file_tool_definition,
    };
    use crate::outbound::agents::mcp::github::tools::validate_scope;
    use reili_core::source_code::github::GithubScopePolicy;

    fn scope_policy() -> GithubScopePolicy {
        GithubScopePolicy::new("acme".to_string()).expect("create scope policy")
    }

    fn line_window(offset: usize, limit: usize) -> LineWindow {
        LineWindow::new(Some(offset), Some(limit))
    }

    fn read_file_arguments(value: Value) -> ReadFileArguments {
        serde_json::from_value(value).expect("deserialize read_file arguments")
    }

    #[test]
    fn definition_advertises_the_read_file_name() {
        assert_eq!(read_file_tool_definition().name, READ_FILE_TOOL_NAME);
    }

    #[test]
    fn validates_owner_scope() {
        let error = validate_scope(
            READ_FILE_TOOL_NAME,
            json!({ "owner": "other", "repo": "svc", "path": "src/main.rs" })
                .as_object()
                .expect("object"),
            &scope_policy(),
        )
        .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[test]
    fn forwarded_arguments_exclude_window_params_and_carry_ref() {
        let arguments = read_file_arguments(json!({
            "owner": "acme",
            "repo": "svc",
            "path": "a.rs",
            "ref": "main",
            "offset": 2,
            "limit": 5,
        }));

        let forwarded = arguments
            .to_forwarded_arguments()
            .expect("forwarded arguments");

        assert_eq!(forwarded.get("owner").and_then(Value::as_str), Some("acme"));
        assert_eq!(forwarded.get("repo").and_then(Value::as_str), Some("svc"));
        assert_eq!(forwarded.get("path").and_then(Value::as_str), Some("a.rs"));
        assert_eq!(forwarded.get("ref").and_then(Value::as_str), Some("main"));
        assert!(!forwarded.contains_key("offset"));
        assert!(!forwarded.contains_key("limit"));

        let window = arguments.line_window();
        assert_eq!(window.offset, 2);
        assert_eq!(window.limit, 5);
    }

    #[test]
    fn forwarded_arguments_omit_absent_ref_and_window_defaults_apply() {
        let arguments = read_file_arguments(json!({
            "owner": "acme",
            "repo": "svc",
            "path": "a.rs",
        }));

        let forwarded = arguments
            .to_forwarded_arguments()
            .expect("forwarded arguments");

        assert!(!forwarded.contains_key("ref"));

        let window = arguments.line_window();
        assert_eq!(window.offset, 1);
        assert_eq!(window.limit, DEFAULT_READ_FILE_LINE_LIMIT);
    }

    #[test]
    fn line_window_treats_non_positive_offset_and_limit_as_defaults() {
        let window = LineWindow::new(Some(0), Some(0));

        assert_eq!(window.offset, 1);
        assert_eq!(window.limit, DEFAULT_READ_FILE_LINE_LIMIT);
    }

    #[test]
    fn apply_numbers_lines_and_appends_range_marker_for_partial_window() {
        let content = "a\nb\nc\nd\ne";

        let result = line_window(2, 2).apply(content);

        assert_eq!(
            result,
            "2\tb\n3\tc\n[showing lines 2-3 of 5; pass offset/limit to read a different range]"
        );
    }

    #[test]
    fn apply_omits_marker_when_whole_file_fits() {
        let content = "a\nb\nc";

        let result = line_window(1, 10).apply(content);

        assert_eq!(result, "1\ta\n2\tb\n3\tc");
    }

    #[test]
    fn apply_reports_when_offset_is_past_end_of_file() {
        let content = "a\nb";

        let result = line_window(5, 10).apply(content);

        assert_eq!(
            result,
            "[no content: requested offset 5 but the file has 2 line(s)]"
        );
    }

    #[test]
    fn extract_file_text_returns_embedded_text_resource() {
        let result = CallToolResult::success(vec![ContentBlock::embedded_text(
            "repo://svc/a.rs",
            "line one\nline two",
        )]);

        assert_eq!(
            extract_file_text(&result).as_deref(),
            Some("line one\nline two")
        );
    }

    #[test]
    fn extract_file_text_returns_none_for_plain_text_directory_listing() {
        let result = CallToolResult::success(vec![ContentBlock::text("[{\"name\":\"a.rs\"}]")]);

        assert!(extract_file_text(&result).is_none());
    }
}
