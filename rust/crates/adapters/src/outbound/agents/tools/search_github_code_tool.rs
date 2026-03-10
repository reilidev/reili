use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{GithubSearchParams, InvestigationResources};

use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchGithubCodeTool {
    resources: Arc<InvestigationResources>,
}

impl SearchGithubCodeTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchGithubCodeArgs {
    pub query: String,
    #[serde(default = "default_search_github_limit")]
    pub limit: u32,
}

fn default_search_github_limit() -> u32 {
    10
}

impl Tool for SearchGithubCodeTool {
    const NAME: &'static str = "search_github_code";

    type Error = PortError;
    type Args = SearchGithubCodeArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Search GitHub code inside the configured organization scope. Every query must explicitly include org:<githubScopeOrg from runtime context>."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "GitHub Code Search query. Always include org:<githubScopeOrg from runtime context>."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 30,
                        "default": 10,
                        "description": "Maximum number of results"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .resources
            .github_search_port
            .search_code(GithubSearchParams {
                query: args.query,
                limit: args.limit,
            })
            .await
        {
            Ok(results) => to_json_string(&results),
            Err(error) => {
                if let Some(soft_error) = to_github_tool_soft_error(&error) {
                    return to_json_string(&soft_error);
                }

                Err(error)
            }
        }
    }
}
