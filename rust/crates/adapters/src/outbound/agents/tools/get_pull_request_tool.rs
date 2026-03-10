use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{GithubPullRequestParams, InvestigationResources};

use super::assert_github_owner_in_scope::assert_github_owner_in_scope;
use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct GetPullRequestTool {
    resources: Arc<InvestigationResources>,
}

impl GetPullRequestTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPullRequestArgs {
    pub owner: String,
    pub repo: String,
    pub pull_number: u64,
}

impl Tool for GetPullRequestTool {
    const NAME: &'static str = "get_pull_request";

    type Error = PortError;
    type Args = GetPullRequestArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Get metadata of a GitHub pull request (state, title, author, changed files count, etc.)."
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
        if let Err(error) =
            assert_github_owner_in_scope(&args.owner, &self.resources.github_scope_org)
        {
            if let Some(soft_error) = to_github_tool_soft_error(&error) {
                return to_json_string(&soft_error);
            }
            return Err(error);
        }

        match self
            .resources
            .github_search_port
            .get_pull_request(GithubPullRequestParams {
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
