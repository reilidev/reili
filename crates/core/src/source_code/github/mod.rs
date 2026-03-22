pub mod pull_request;
pub mod repository_content;
pub mod search;

pub use pull_request::{
    GithubPullRequestDiff, GithubPullRequestParams, GithubPullRequestPort, GithubPullRequestSummary,
};
pub use repository_content::{
    GithubRepositoryContent, GithubRepositoryContentParams, GithubRepositoryContentPort,
    GithubRepositoryDirectoryContent, GithubRepositoryDirectoryEntry, GithubRepositoryFileContent,
    GithubRepositoryFileEncoding,
};
pub use search::{
    GithubCodeSearchPort, GithubCodeSearchResultItem, GithubIssueSearchResultItem,
    GithubRepoSearchResultItem, GithubSearchParams,
};

#[cfg(any(test, feature = "test-support"))]
pub use pull_request::MockGithubPullRequestPort;
#[cfg(any(test, feature = "test-support"))]
pub use repository_content::MockGithubRepositoryContentPort;
#[cfg(any(test, feature = "test-support"))]
pub use search::MockGithubCodeSearchPort;
