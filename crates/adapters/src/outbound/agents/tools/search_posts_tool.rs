use std::sync::Arc;

use reili_core::error::PortError;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::outbound::esa::{
    EsaPostSearchInput, EsaPostSearchOrder, EsaPostSearchPort, EsaPostSearchSort,
};

use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchPostsTool {
    esa_post_search_port: Arc<dyn EsaPostSearchPort>,
}

impl SearchPostsTool {
    pub fn new(esa_post_search_port: Arc<dyn EsaPostSearchPort>) -> Self {
        Self {
            esa_post_search_port,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPostsArgs {
    pub q: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
    #[serde(default = "default_sort")]
    pub sort: EsaPostSearchSort,
    #[serde(default = "default_order")]
    pub order: EsaPostSearchOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct EsaToolSoftError {
    error_type: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_error_code: Option<String>,
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    5
}

fn default_sort() -> EsaPostSearchSort {
    EsaPostSearchSort::BestMatch
}

fn default_order() -> EsaPostSearchOrder {
    EsaPostSearchOrder::Desc
}

impl Tool for SearchPostsTool {
    const NAME: &'static str = "search_posts";

    type Error = PortError;
    type Args = SearchPostsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search the configured esa knowledge base for internal documents, runbooks, investigation notes, and operational knowledge. The q field must use esa post search query syntax.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "q": {
                        "type": "string",
                        "description": "esa search query. Supports esa syntax such as title:, body:, category:, in:, on:, tag:, #tag, @screen_name, user:, updated_by:, comment:, starred:true, watched:true, sharing:true, stars:>3, created:>YYYY-MM-DD, updated:>YYYY-MM, AND by spaces, OR, |, -keyword, and parentheses.",
                        "maxLength": 1000
                    },
                    "page": {
                        "type": "integer",
                        "minimum": 1,
                        "default": 1,
                        "description": "1-based result page."
                    },
                    "perPage": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "default": 5,
                        "description": "Maximum number of posts to return."
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["updated", "created", "number", "stars", "watches", "comments", "best_match"],
                        "default": "best_match",
                        "description": "esa post result sort key."
                    },
                    "order": {
                        "type": "string",
                        "enum": ["desc", "asc"],
                        "default": "desc",
                        "description": "Result sort order."
                    }
                },
                "required": ["q"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .esa_post_search_port
            .search_posts(EsaPostSearchInput {
                q: args.q,
                page: args.page,
                per_page: args.per_page,
                sort: args.sort,
                order: args.order,
            })
            .await
        {
            Ok(result) => to_json_string(&result),
            Err(error) => to_json_string(&to_esa_tool_soft_error(&error)),
        }
    }
}

fn to_esa_tool_soft_error(error: &PortError) -> EsaToolSoftError {
    let status_code = error.status_code();
    let (error_type, retryable) = match status_code {
        Some(401 | 403) => ("authorization_error", false),
        Some(429) => ("rate_limited", true),
        Some(status) if status >= 500 => ("upstream_error", true),
        Some(status) if status >= 400 => ("request_error", false),
        _ if error.is_invalid_input() => ("invalid_input", false),
        _ if error.is_connection_failed() => ("temporary_error", true),
        _ => ("tool_error", false),
    };

    EsaToolSoftError {
        error_type: error_type.to_string(),
        message: error.message.clone(),
        retryable,
        status_code,
        service_error_code: error.service_error_code().map(ToString::to_string),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::outbound::esa::{
        EsaPost, EsaPostSearchInput, EsaPostSearchOrder, EsaPostSearchPort, EsaPostSearchResult,
        EsaPostSearchSort,
    };
    use async_trait::async_trait;
    use reili_core::error::PortError;
    use rig::tool::Tool;

    use super::{SearchPostsArgs, SearchPostsTool};

    struct MockEsaPostSearchPort {
        calls: Arc<Mutex<Vec<EsaPostSearchInput>>>,
        result: Result<EsaPostSearchResult, PortError>,
    }

    #[async_trait]
    impl EsaPostSearchPort for MockEsaPostSearchPort {
        async fn search_posts(
            &self,
            input: EsaPostSearchInput,
        ) -> Result<EsaPostSearchResult, PortError> {
            self.calls.lock().expect("lock calls").push(input);
            self.result.clone()
        }
    }

    fn build_tool(
        calls: Arc<Mutex<Vec<EsaPostSearchInput>>>,
        result: Result<EsaPostSearchResult, PortError>,
    ) -> SearchPostsTool {
        SearchPostsTool::new(Arc::new(MockEsaPostSearchPort { calls, result }))
    }

    #[tokio::test]
    async fn converts_args_to_search_input_and_returns_json() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            Ok(EsaPostSearchResult {
                posts: vec![EsaPost {
                    number: 42,
                    name: "Runbook".to_string(),
                    wip: false,
                    body_md: "# Runbook".to_string(),
                    url: Some("https://docs.esa.io/posts/42".to_string()),
                    category: Some("SRE".to_string()),
                    tags: vec!["alert".to_string()],
                    created_at: None,
                    updated_at: None,
                    created_by: None,
                    updated_by: None,
                    comments_count: Some(1),
                    watchers_count: None,
                }],
                prev_page: None,
                next_page: None,
                total_count: 1,
                page: 1,
                per_page: 5,
                max_per_page: 100,
            }),
        );

        let output = tool
            .call(SearchPostsArgs {
                q: "in:runbooks error".to_string(),
                page: 1,
                per_page: 5,
                sort: EsaPostSearchSort::BestMatch,
                order: EsaPostSearchOrder::Desc,
            })
            .await
            .expect("call search_posts");

        assert!(output.contains("Runbook"));
        assert!(output.contains("https://docs.esa.io/posts/42"));

        let captured = calls.lock().expect("lock calls");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].q, "in:runbooks error");
        assert_eq!(captured[0].sort, EsaPostSearchSort::BestMatch);
    }

    #[tokio::test]
    async fn returns_soft_error_json_when_search_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            Err(PortError::http_status(
                429,
                "esa API request failed: retryAfter=30",
            )),
        );

        let output = tool
            .call(SearchPostsArgs {
                q: "runbook".to_string(),
                page: 1,
                per_page: 5,
                sort: EsaPostSearchSort::Updated,
                order: EsaPostSearchOrder::Desc,
            })
            .await
            .expect("call search_posts");

        assert!(output.contains("rate_limited"));
        assert!(output.contains("\"retryable\":true"));
        assert!(output.contains("\"statusCode\":429"));
        assert_eq!(calls.lock().expect("lock calls").len(), 1);
    }

    #[tokio::test]
    async fn tool_schema_has_required_q_and_esa_sort_values() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            calls,
            Ok(EsaPostSearchResult {
                posts: Vec::new(),
                prev_page: None,
                next_page: None,
                total_count: 0,
                page: 1,
                per_page: 5,
                max_per_page: 100,
            }),
        );

        let definition = tool.definition("test".to_string()).await;
        assert_eq!(definition.name, "search_posts");
        let required = definition.parameters["required"]
            .as_array()
            .expect("required array");
        assert!(required.contains(&serde_json::Value::String("q".to_string())));
        let sort_values = definition.parameters["properties"]["sort"]["enum"]
            .as_array()
            .expect("sort enum");
        assert!(sort_values.contains(&serde_json::Value::String("best_match".to_string())));
        assert_eq!(
            definition.parameters["properties"]["perPage"]["maximum"],
            serde_json::Value::from(10)
        );
    }
}
