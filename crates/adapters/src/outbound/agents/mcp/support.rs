use std::collections::HashSet;
use std::io;

use rig::completion::ToolDefinition;
use rig::tool::ToolError;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use serde_json::{Map, Value};
use tracing::error;

use crate::outbound::mcp_streamable_http::{McpHttpClientAuth, StreamableHttpMcpClient};

pub(super) fn mcp_tool_definition(tool: &Tool) -> ToolDefinition {
    ToolDefinition {
        name: tool.name.to_string(),
        description: tool.description.clone().unwrap_or_default().to_string(),
        parameters: serde_json::to_value(&tool.input_schema).unwrap_or_default(),
    }
}

pub(super) fn parse_tool_arguments(
    source_label: &str,
    args: &str,
) -> Result<Map<String, Value>, ToolError> {
    serde_json::from_str::<Value>(args)?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            ToolError::ToolCallError(Box::new(io::Error::other(format!(
                "{source_label} MCP tool arguments must be a JSON object"
            ))))
        })
}

pub(super) fn filter_tools_by_name(tools: &[Tool], names: &[&str]) -> Vec<Tool> {
    let expected_names: HashSet<&str> = names.iter().copied().collect();

    tools
        .iter()
        .filter(|tool| expected_names.contains(tool.name.as_ref()))
        .cloned()
        .collect()
}

/// Distinguishes a transport failure from an `is_error` result, logging and wrapping either as a `ToolError`.
pub(super) async fn call_mcp_tool<A: McpHttpClientAuth + Clone>(
    source_label: &str,
    client: &StreamableHttpMcpClient<A>,
    name: &str,
    arguments: Map<String, Value>,
) -> Result<CallToolResult, ToolError> {
    let result = client
        .call_tool(name.to_string(), Some(arguments))
        .await
        .map_err(|transport_error| {
            let error_message = format!(
                "{source_label} MCP tool {name} failed before returning a result: {transport_error}"
            );
            error!(tool_name = %name, error = %transport_error, "{error_message}");
            ToolError::ToolCallError(Box::new(io::Error::other(error_message)))
        })?;

    if matches!(result.is_error, Some(true)) {
        let error_message = format_tool_error(source_label, name, &result);
        error!(
            tool_name = %name,
            error_message = %error_message,
            structured_content = ?result.structured_content,
            content = ?result.content,
            "{source_label} MCP tool returned an error"
        );
        return Err(ToolError::ToolCallError(Box::new(io::Error::other(
            error_message,
        ))));
    }

    Ok(result)
}

/// Falls back to `structured_content` when the result has no text content blocks.
pub(super) fn format_tool_success(result: &CallToolResult) -> String {
    let content = render_contents(&result.content);
    if !content.is_empty() {
        return content;
    }

    result
        .structured_content
        .as_ref()
        .map_or_else(String::new, serde_json::Value::to_string)
}

/// Combines content, structured_content, and meta into one diagnostic string, in that order.
pub(super) fn format_tool_error(
    source_label: &str,
    tool_name: &str,
    result: &CallToolResult,
) -> String {
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
        "{source_label} MCP tool {tool_name} returned an error: {}",
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

/// `char_limit` and `hint` are caller-specific; the truncation marker shape is shared.
pub(super) fn truncate_content(content: String, char_limit: usize, hint: &str) -> String {
    if content.len() <= char_limit {
        return content;
    }
    let truncated = &content[..char_limit];
    let returned_lines = truncated.lines().count();
    let total_lines = content.lines().count();
    format!("{truncated}\n[truncated: {returned_lines} of {total_lines} lines shown; {hint}]")
}

#[cfg(test)]
mod tests {
    use rmcp::model::{CallToolResult, ContentBlock, Tool};
    use serde_json::json;

    use super::{
        filter_tools_by_name, format_tool_error, format_tool_success, mcp_tool_definition,
        parse_tool_arguments, truncate_content,
    };

    fn tool(name: &str) -> Tool {
        Tool::new(name.to_string(), "test tool", serde_json::Map::new())
    }

    #[test]
    fn builds_tool_definition_from_rmcp_tool() {
        let definition = mcp_tool_definition(&tool("search_code"));

        assert_eq!(definition.name, "search_code");
        assert_eq!(definition.description, "test tool");
    }

    #[test]
    fn parses_object_arguments() {
        let arguments = parse_tool_arguments("Example", r#"{"query":"foo"}"#).expect("parse");

        assert_eq!(
            arguments.get("query").and_then(serde_json::Value::as_str),
            Some("foo")
        );
    }

    #[test]
    fn rejects_non_object_arguments() {
        let error = parse_tool_arguments("Example", "[1,2,3]").expect_err("non-object should fail");

        assert!(error.to_string().contains("Example MCP tool arguments"));
    }

    #[test]
    fn filters_tools_by_name() {
        let tools = vec![tool("a"), tool("b"), tool("c")];

        let filtered = filter_tools_by_name(&tools, &["a", "c"]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name.as_ref(), "a");
        assert_eq!(filtered[1].name.as_ref(), "c");
    }

    #[test]
    fn formats_success_from_structured_content_when_text_content_is_empty() {
        let mut result = CallToolResult::success(vec![]);
        result.structured_content = Some(json!({"status": "ok"}));

        assert_eq!(format_tool_success(&result), "{\"status\":\"ok\"}");
    }

    #[test]
    fn formats_tool_error_with_text_and_structured_content() {
        let mut result = CallToolResult::error(vec![ContentBlock::text("request failed")]);
        result.structured_content = Some(json!({
            "error_code": "FORBIDDEN",
            "details": "permission denied"
        }));

        assert_eq!(
            format_tool_error("Example", "search_logs", &result),
            "Example MCP tool search_logs returned an error: content=request failed; structured_content={\"details\":\"permission denied\",\"error_code\":\"FORBIDDEN\"}"
        );
    }

    #[test]
    fn formats_tool_error_from_embedded_text_resource() {
        let result = CallToolResult::error(vec![ContentBlock::embedded_text(
            "example://error",
            "resource failure",
        )]);

        assert_eq!(
            format_tool_error("Example", "search_metrics", &result),
            "Example MCP tool search_metrics returned an error: content=resource failure"
        );
    }

    #[test]
    fn does_not_truncate_content_within_limit() {
        let content = "a".repeat(20);
        assert_eq!(
            truncate_content(content.clone(), 20, "narrow the query"),
            content
        );
    }

    #[test]
    fn truncates_content_exceeding_limit_and_appends_hint() {
        let line = "x".repeat(100) + "\n";
        let content = line.repeat(50);
        let result = truncate_content(content.clone(), 1000, "narrow the query");

        assert!(result.len() < content.len());
        assert!(result.contains("[truncated:"));
        assert!(result.contains("narrow the query"));
    }
}
