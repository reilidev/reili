use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::source_code::github::{GithubPullRequestParams, GithubPullRequestPort};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct GetPullRequestTool {
    github_pull_request_port: Arc<dyn GithubPullRequestPort>,
}

impl GetPullRequestTool {
    pub fn new(github_pull_request_port: Arc<dyn GithubPullRequestPort>) -> Self {
        Self {
            github_pull_request_port,
        }
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
        match self
            .github_pull_request_port
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::source_code::github::{GithubPullRequestPort, MockGithubPullRequestPort};
    use rig::tool::Tool;
    use serde_json::json;

    use super::{GetPullRequestArgs, GetPullRequestTool};
    use reili_core::error::PortError;

    #[tokio::test]
    async fn converts_invalid_input_from_port_into_soft_error_json() {
        let mut github_pull_request_port = MockGithubPullRequestPort::new();
        github_pull_request_port
            .expect_get_pull_request()
            .once()
            .return_once(|_| {
                Err(PortError::invalid_input(
                    "owner is out of scope. allowed owner: acme",
                ))
            });

        let tool = GetPullRequestTool::new(
            Arc::new(github_pull_request_port) as Arc<dyn GithubPullRequestPort>
        );

        let result = tool
            .call(GetPullRequestArgs {
                owner: "other-org".to_string(),
                repo: "service".to_string(),
                pull_number: 42,
            })
            .await
            .expect("tool should return soft error");

        let actual: serde_json::Value =
            serde_json::from_str(&result).expect("deserialize tool result");

        assert_eq!(
            actual,
            json!({
                "ok": false,
                "kind": "client_error",
                "message": "owner is out of scope. allowed owner: acme"
            })
        );
    }
}
