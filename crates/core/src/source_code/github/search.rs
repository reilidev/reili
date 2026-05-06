use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSearchParams {
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubCodeSearchResultItem {
    pub name: String,
    pub path: String,
    pub repository_full_name: String,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRepoSearchResultItem {
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub default_branch: String,
    pub language: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubIssueSearchResultItem {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub repository_url: String,
    pub user_login: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub pull_request: bool,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait GithubCodeSearchPort: Send + Sync {
    async fn search_code(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubCodeSearchResultItem>, PortError>;

    async fn search_repos(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubRepoSearchResultItem>, PortError>;

    async fn search_issues_and_pull_requests(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubIssueSearchResultItem>, PortError>;
}
