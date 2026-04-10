use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::knowledge::{WebSearchInput, WebSearchUserLocation};
use reili_core::task::TaskResources;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchWebTool {
    resources: Arc<TaskResources>,
}

impl SearchWebTool {
    pub fn new(resources: Arc<TaskResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchWebArgs {
    pub query: String,
    #[serde(default)]
    pub timezone: String,
}

impl Tool for SearchWebTool {
    const NAME: &'static str = "search_web";

    type Error = PortError;
    type Args = SearchWebArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search the public web for recent information relevant to the investigation. Returns a summary with source citations.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query. Max 500 characters.",
                        "maxLength": 500
                    },
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone for search location context (e.g. Asia/Tokyo)."
                    }
                },
                "required": ["query","timezone"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let input = WebSearchInput {
            query: args.query,
            user_location: WebSearchUserLocation {
                timezone: args.timezone,
            },
        };

        let result = self.resources.web_search_port.search(input).await?;
        to_json_string(&result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::knowledge::{WebCitation, WebSearchInput, WebSearchPort, WebSearchResult};
    use rig::tool::Tool;

    use super::*;
    use crate::outbound::agents::tools::search_web_tool::SearchWebArgs;

    struct MockWebSearchPort {
        calls: Arc<Mutex<Vec<WebSearchInput>>>,
        result: WebSearchResult,
    }

    #[async_trait]
    impl WebSearchPort for MockWebSearchPort {
        async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError> {
            self.calls.lock().expect("lock calls").push(input);
            Ok(self.result.clone())
        }
    }

    fn build_tool(
        calls: Arc<Mutex<Vec<WebSearchInput>>>,
        result: WebSearchResult,
    ) -> SearchWebTool {
        let mock_port = Arc::new(MockWebSearchPort { calls, result });
        let resources = Arc::new(build_test_resources(mock_port));
        SearchWebTool::new(resources)
    }

    fn build_test_resources(web_search_port: Arc<dyn WebSearchPort>) -> TaskResources {
        use reili_core::messaging::slack::{
            SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchResult,
        };

        struct StubSlackMessageSearch;
        #[async_trait]
        impl SlackMessageSearchPort for StubSlackMessageSearch {
            async fn search_messages(
                &self,
                _: SlackMessageSearchInput,
            ) -> Result<SlackMessageSearchResult, PortError> {
                unimplemented!()
            }
        }

        TaskResources {
            slack_message_search_port: Arc::new(StubSlackMessageSearch),
            web_search_port,
        }
    }

    #[tokio::test]
    async fn converts_args_to_web_search_input_and_returns_json() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            WebSearchResult {
                summary_text: "Summary".to_string(),
                citations: vec![WebCitation {
                    title: "Title".to_string(),
                    url: "https://example.com".to_string(),
                }],
            },
        );

        let output = tool
            .call(SearchWebArgs {
                query: "test query".to_string(),
                timezone: "Asia/Tokyo".to_string(),
            })
            .await
            .expect("call search_web");

        assert!(output.contains("Summary"));
        assert!(output.contains("https://example.com"));

        let captured = calls.lock().expect("lock calls");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].query, "test query");
    }

    #[tokio::test]
    async fn passes_soft_error_through() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            WebSearchResult {
                summary_text: "{\"type\":\"temporary_error\",\"message\":\"timed out\"}"
                    .to_string(),
                citations: Vec::new(),
            },
        );

        let output = tool
            .call(SearchWebArgs {
                query: "test".to_string(),
                timezone: String::new(),
            })
            .await
            .expect("call search_web");

        assert!(output.contains("temporary_error"));
    }

    #[tokio::test]
    async fn tool_schema_has_required_query() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            calls,
            WebSearchResult {
                summary_text: String::new(),
                citations: Vec::new(),
            },
        );

        let definition = tool.definition("test".to_string()).await;
        assert_eq!(definition.name, "search_web");
        let required = definition.parameters["required"]
            .as_array()
            .expect("required array");
        assert!(required.contains(&serde_json::Value::String("query".to_string())));
    }
}
