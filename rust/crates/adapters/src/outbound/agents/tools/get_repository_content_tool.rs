use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{GithubRepositoryContentParams, InvestigationResources};

use super::assert_github_owner_in_scope::assert_github_owner_in_scope;
use super::github_tool_soft_error::to_github_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct GetRepositoryContentTool {
    resources: Arc<InvestigationResources>,
}

impl GetRepositoryContentTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
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
