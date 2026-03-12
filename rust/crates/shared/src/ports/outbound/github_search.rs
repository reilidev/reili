use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSearchParams {
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositoryContentParams {
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestParams {
    pub owner: String,
    pub repo: String,
    pub pull_number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubCodeSearchResultItem {
    pub name: String,
    pub path: String,
    pub repository_full_name: String,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepoSearchResultItem {
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub default_branch: String,
    pub language: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositoryFileContent {
    pub content: String,
    pub encoding: GithubRepositoryFileEncoding,
    pub html_url: String,
    pub original_bytes: u64,
    pub returned_chars: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GithubRepositoryFileEncoding {
    Utf8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositoryDirectoryEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepositoryDirectoryContent {
    pub html_url: String,
    pub entries: Vec<GithubRepositoryDirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GithubRepositoryContent {
    File(GithubRepositoryFileContent),
    Directory(GithubRepositoryDirectoryContent),
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

#[async_trait]
pub trait GithubRepositoryContentPort: Send + Sync {
    async fn get_repository_content(
        &self,
        params: GithubRepositoryContentParams,
    ) -> Result<GithubRepositoryContent, PortError>;
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

#[cfg(test)]
mod tests {
    use super::{
        GithubRepositoryContent, GithubRepositoryFileContent, GithubRepositoryFileEncoding,
    };

    #[test]
    fn serializes_and_deserializes_repository_content() {
        let value = GithubRepositoryContent::File(GithubRepositoryFileContent {
            content: "fn main() {}".to_string(),
            encoding: GithubRepositoryFileEncoding::Utf8,
            html_url: "https://example.com".to_string(),
            original_bytes: 12,
            returned_chars: 12,
            truncated: false,
        });

        let json = serde_json::to_string(&value).expect("serialize github repository content");
        let restored: GithubRepositoryContent =
            serde_json::from_str(&json).expect("deserialize github repository content");

        assert_eq!(restored, value);
    }
}
