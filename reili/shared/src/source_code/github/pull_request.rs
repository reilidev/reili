use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestParams {
    pub owner: String,
    pub repo: String,
    pub pull_number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestDiff {
    pub diff: String,
    pub html_url: String,
    pub original_chars: u64,
    pub returned_chars: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestSummary {
    pub number: u64,
    pub state: String,
    pub title: String,
    pub body: Option<String>,
    pub user_login: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub merged_at: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changed_files: Option<u64>,
    pub commits: Option<u64>,
    pub html_url: String,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
}

#[async_trait]
pub trait GithubPullRequestPort: Send + Sync {
    async fn get_pull_request(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestSummary, PortError>;

    async fn get_pull_request_diff(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestDiff, PortError>;
}
