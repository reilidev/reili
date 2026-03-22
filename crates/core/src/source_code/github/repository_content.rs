use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

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

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait GithubRepositoryContentPort: Send + Sync {
    async fn get_repository_content(
        &self,
        params: GithubRepositoryContentParams,
    ) -> Result<GithubRepositoryContent, PortError>;
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
