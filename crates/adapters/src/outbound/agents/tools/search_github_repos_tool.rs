use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::source_code::github::{GithubCodeSearchPort, GithubSearchParams};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchGithubReposTool {
    github_code_search_port: Arc<dyn GithubCodeSearchPort>,
    github_scope_org: String,
}

impl SearchGithubReposTool {
    pub fn new(
        github_code_search_port: Arc<dyn GithubCodeSearchPort>,
        github_scope_org: String,
    ) -> Self {
        Self {
            github_code_search_port,
            github_scope_org,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchGithubReposArgs {
    pub query: String,
    #[serde(default = "default_search_github_limit")]
    pub limit: u32,
}

fn default_search_github_limit() -> u32 {
    10
}

impl Tool for SearchGithubReposTool {
    const NAME: &'static str = "search_github_repos";

    type Error = PortError;
    type Args = SearchGithubReposArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: format!(
                "Search GitHub repositories inside the configured organization scope ({scope_org}). Every query must explicitly include org:{scope_org}.",
                scope_org = self.github_scope_org
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": format!("GitHub repository search query. Always include org:{}.", self.github_scope_org)
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
            .github_code_search_port
            .search_repos(GithubSearchParams {
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
