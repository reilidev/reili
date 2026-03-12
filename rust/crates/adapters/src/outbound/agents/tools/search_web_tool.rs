use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{InvestigationResources, WebSearchInput, WebSearchUserLocation};

use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchWebTool {
    resources: Arc<InvestigationResources>,
}

impl SearchWebTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
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
    use rig::tool::Tool;
    use sre_shared::errors::PortError;
    use sre_shared::ports::outbound::{
        WebCitation, WebSearchExecution, WebSearchInput, WebSearchPort, WebSearchResult,
    };

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

    fn build_test_resources(web_search_port: Arc<dyn WebSearchPort>) -> InvestigationResources {
        use sre_shared::ports::outbound::{
            DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
            DatadogMetricCatalogPort, DatadogMetricQueryPort, GithubCodeSearchPort,
            GithubPullRequestPort, GithubRepositoryContentPort,
        };

        struct StubLogAggregate;
        #[async_trait]
        impl DatadogLogAggregatePort for StubLogAggregate {
            async fn aggregate_by_facet(
                &self,
                _: sre_shared::ports::outbound::DatadogLogAggregateParams,
            ) -> Result<Vec<sre_shared::ports::outbound::DatadogLogAggregateBucket>, PortError>
            {
                Ok(Vec::new())
            }
        }

        struct StubLogSearch;
        #[async_trait]
        impl DatadogLogSearchPort for StubLogSearch {
            async fn search_logs(
                &self,
                _: sre_shared::ports::outbound::DatadogLogSearchParams,
            ) -> Result<Vec<sre_shared::ports::outbound::DatadogLogSearchResult>, PortError>
            {
                unimplemented!()
            }
        }

        struct StubMetricCatalog;
        #[async_trait]
        impl DatadogMetricCatalogPort for StubMetricCatalog {
            async fn list_metrics(
                &self,
                _: sre_shared::ports::outbound::DatadogMetricCatalogParams,
            ) -> Result<Vec<String>, PortError> {
                Ok(Vec::new())
            }
        }

        struct StubMetricQuery;
        #[async_trait]
        impl DatadogMetricQueryPort for StubMetricQuery {
            async fn query_metrics(
                &self,
                _: sre_shared::ports::outbound::DatadogMetricQueryParams,
            ) -> Result<Vec<sre_shared::ports::outbound::DatadogMetricQueryResult>, PortError>
            {
                unimplemented!()
            }
        }

        struct StubEventSearch;
        #[async_trait]
        impl DatadogEventSearchPort for StubEventSearch {
            async fn search_events(
                &self,
                _: sre_shared::ports::outbound::DatadogEventSearchParams,
            ) -> Result<Vec<sre_shared::ports::outbound::DatadogEventSearchResult>, PortError>
            {
                unimplemented!()
            }
        }

        struct StubGithubSearch;
        #[async_trait]
        impl GithubCodeSearchPort for StubGithubSearch {
            async fn search_code(
                &self,
                _: sre_shared::ports::outbound::GithubSearchParams,
            ) -> Result<Vec<sre_shared::ports::outbound::GithubCodeSearchResultItem>, PortError>
            {
                unimplemented!()
            }
            async fn search_repos(
                &self,
                _: sre_shared::ports::outbound::GithubSearchParams,
            ) -> Result<Vec<sre_shared::ports::outbound::GithubRepoSearchResultItem>, PortError>
            {
                unimplemented!()
            }
            async fn search_issues_and_pull_requests(
                &self,
                _: sre_shared::ports::outbound::GithubSearchParams,
            ) -> Result<Vec<sre_shared::ports::outbound::GithubIssueSearchResultItem>, PortError>
            {
                unimplemented!()
            }
        }

        #[async_trait]
        impl GithubRepositoryContentPort for StubGithubSearch {
            async fn get_repository_content(
                &self,
                _: sre_shared::ports::outbound::GithubRepositoryContentParams,
            ) -> Result<sre_shared::ports::outbound::GithubRepositoryContent, PortError>
            {
                unimplemented!()
            }
        }

        #[async_trait]
        impl GithubPullRequestPort for StubGithubSearch {
            async fn get_pull_request(
                &self,
                _: sre_shared::ports::outbound::GithubPullRequestParams,
            ) -> Result<sre_shared::ports::outbound::GithubPullRequestSummary, PortError>
            {
                unimplemented!()
            }
            async fn get_pull_request_diff(
                &self,
                _: sre_shared::ports::outbound::GithubPullRequestParams,
            ) -> Result<sre_shared::ports::outbound::GithubPullRequestDiff, PortError> {
                unimplemented!()
            }
        }

        InvestigationResources {
            log_aggregate_port: Arc::new(StubLogAggregate),
            log_search_port: Arc::new(StubLogSearch),
            metric_catalog_port: Arc::new(StubMetricCatalog),
            metric_query_port: Arc::new(StubMetricQuery),
            event_search_port: Arc::new(StubEventSearch),
            github_code_search_port: Arc::new(StubGithubSearch),
            github_repository_content_port: Arc::new(StubGithubSearch),
            github_pull_request_port: Arc::new(StubGithubSearch),
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
                searches: vec![WebSearchExecution {
                    query: "test".to_string(),
                    source_count: 1,
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
                searches: Vec::new(),
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
                searches: Vec::new(),
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
