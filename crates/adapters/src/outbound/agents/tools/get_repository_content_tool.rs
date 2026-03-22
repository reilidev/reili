use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::source_code::github::{GithubRepositoryContentParams, GithubRepositoryContentPort};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct GetRepositoryContentTool {
    github_repository_content_port: Arc<dyn GithubRepositoryContentPort>,
}

impl GetRepositoryContentTool {
    pub fn new(github_repository_content_port: Arc<dyn GithubRepositoryContentPort>) -> Self {
        Self {
            github_repository_content_port,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRepositoryContentArgs {
    pub owner: String,
    pub repo: String,
    pub path: String,
    #[serde(rename = "ref", default)]
    pub git_ref: Option<String>,
}

impl Tool for GetRepositoryContentTool {
    const NAME: &'static str = "get_repository_content";

    type Error = PortError;
    type Args = GetRepositoryContentArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Retrieve repository content in configured organization scope. Returns kind=file|directory and truncates oversized file content."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "owner": {
                        "type": "string",
                        "description": "Repository owner (must match configured organization)"
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository name"
                    },
                    "path": {
                        "type": "string",
                        "description": "File path within the repository"
                    },
                    "ref": {
                        "type": ["string", "null"],
                        "default": null,
                        "description": "Git ref (branch, tag, or commit SHA). Defaults to default branch."
                    }
                },
                "required": ["owner", "repo", "path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .github_repository_content_port
            .get_repository_content(GithubRepositoryContentParams {
                owner: args.owner,
                repo: args.repo,
                path: args.path,
                r#ref: args.git_ref,
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

    use reili_core::source_code::github::{
        GithubRepositoryContentPort, MockGithubRepositoryContentPort,
    };
    use rig::tool::Tool;
    use serde_json::json;

    use super::{GetRepositoryContentArgs, GetRepositoryContentTool};
    use reili_core::error::PortError;

    #[tokio::test]
    async fn converts_invalid_input_from_port_into_soft_error_json() {
        let mut github_repository_content_port = MockGithubRepositoryContentPort::new();
        github_repository_content_port
            .expect_get_repository_content()
            .once()
            .return_once(|_| {
                Err(PortError::invalid_input(
                    "owner is out of scope. allowed owner: acme",
                ))
            });

        let tool = GetRepositoryContentTool::new(
            Arc::new(github_repository_content_port) as Arc<dyn GithubRepositoryContentPort>
        );

        let result = tool
            .call(GetRepositoryContentArgs {
                owner: "other-org".to_string(),
                repo: "service".to_string(),
                path: "src/lib.rs".to_string(),
                git_ref: None,
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
