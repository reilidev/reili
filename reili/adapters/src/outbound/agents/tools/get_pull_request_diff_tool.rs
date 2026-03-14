use std::sync::Arc;

use reili_shared::error::PortError;
use reili_shared::source_code::github::{GithubPullRequestParams, GithubPullRequestPort};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::assert_github_owner_in_scope::assert_github_owner_in_scope;
use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct GetPullRequestDiffTool {
    github_pull_request_port: Arc<dyn GithubPullRequestPort>,
    github_scope_org: String,
}

impl GetPullRequestDiffTool {
    pub fn new(
        github_pull_request_port: Arc<dyn GithubPullRequestPort>,
        github_scope_org: String,
    ) -> Self {
        Self {
            github_pull_request_port,
            github_scope_org,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPullRequestDiffArgs {
    pub owner: String,
    pub repo: String,
    pub pull_number: u64,
}

impl Tool for GetPullRequestDiffTool {
    const NAME: &'static str = "get_pull_request_diff";

    type Error = PortError;
    type Args = GetPullRequestDiffArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Get the code diff of a GitHub pull request in configured organization scope. Diff is truncated when too large; use sparingly after narrowing scope."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "pullNumber": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Pull request number"
                    }
                },
                "required": ["owner", "repo", "pullNumber"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(error) = assert_github_owner_in_scope(&args.owner, &self.github_scope_org) {
            if let Some(soft_error) = to_github_tool_soft_error(&error) {
                return to_json_string(&soft_error);
            }
            return Err(error);
        }

        match self
            .github_pull_request_port
            .get_pull_request_diff(GithubPullRequestParams {
                owner: args.owner,
                repo: args.repo,
                pull_number: args.pull_number,
            })
            .await
        {
            Ok(result) => to_json_string(&result),
            Err(error) => {
                if let Some(soft_error) = to_github_tool_soft_error(&error) {
                    return to_json_string(&soft_error);
                }

                Err(error)
            }
        }
    }
}
