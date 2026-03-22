use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::source_code::github::{
    GithubCodeSearchPort, GithubCodeSearchResultItem, GithubIssueSearchResultItem,
    GithubPullRequestDiff, GithubPullRequestParams, GithubPullRequestPort,
    GithubPullRequestSummary, GithubRepoSearchResultItem, GithubRepositoryContent,
    GithubRepositoryContentParams, GithubRepositoryContentPort, GithubScopePolicy,
    GithubSearchParams,
};

#[derive(Clone)]
pub struct ScopedGithubCodeSearchPort {
    inner: Arc<dyn GithubCodeSearchPort>,
    scope_policy: Arc<GithubScopePolicy>,
}

impl ScopedGithubCodeSearchPort {
    pub fn new(inner: Arc<dyn GithubCodeSearchPort>, scope_policy: Arc<GithubScopePolicy>) -> Self {
        Self {
            inner,
            scope_policy,
        }
    }
}

#[async_trait]
impl GithubCodeSearchPort for ScopedGithubCodeSearchPort {
    async fn search_code(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubCodeSearchResultItem>, PortError> {
        self.scope_policy.validate_query(&params.query)?;
        self.inner.search_code(params).await
    }

    async fn search_repos(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubRepoSearchResultItem>, PortError> {
        self.scope_policy.validate_query(&params.query)?;
        self.inner.search_repos(params).await
    }

    async fn search_issues_and_pull_requests(
        &self,
        params: GithubSearchParams,
    ) -> Result<Vec<GithubIssueSearchResultItem>, PortError> {
        self.scope_policy.validate_query(&params.query)?;
        self.inner.search_issues_and_pull_requests(params).await
    }
}

#[derive(Clone)]
pub struct ScopedGithubRepositoryContentPort {
    inner: Arc<dyn GithubRepositoryContentPort>,
    scope_policy: Arc<GithubScopePolicy>,
}

impl ScopedGithubRepositoryContentPort {
    pub fn new(
        inner: Arc<dyn GithubRepositoryContentPort>,
        scope_policy: Arc<GithubScopePolicy>,
    ) -> Self {
        Self {
            inner,
            scope_policy,
        }
    }
}

#[async_trait]
impl GithubRepositoryContentPort for ScopedGithubRepositoryContentPort {
    async fn get_repository_content(
        &self,
        params: GithubRepositoryContentParams,
    ) -> Result<GithubRepositoryContent, PortError> {
        self.scope_policy.validate_owner(&params.owner)?;
        self.inner.get_repository_content(params).await
    }
}

#[derive(Clone)]
pub struct ScopedGithubPullRequestPort {
    inner: Arc<dyn GithubPullRequestPort>,
    scope_policy: Arc<GithubScopePolicy>,
}

impl ScopedGithubPullRequestPort {
    pub fn new(
        inner: Arc<dyn GithubPullRequestPort>,
        scope_policy: Arc<GithubScopePolicy>,
    ) -> Self {
        Self {
            inner,
            scope_policy,
        }
    }
}

#[async_trait]
impl GithubPullRequestPort for ScopedGithubPullRequestPort {
    async fn get_pull_request(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestSummary, PortError> {
        self.scope_policy.validate_owner(&params.owner)?;
        self.inner.get_pull_request(params).await
    }

    async fn get_pull_request_diff(
        &self,
        params: GithubPullRequestParams,
    ) -> Result<GithubPullRequestDiff, PortError> {
        self.scope_policy.validate_owner(&params.owner)?;
        self.inner.get_pull_request_diff(params).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::source_code::github::{
        GithubCodeSearchResultItem, GithubPullRequestDiff, GithubPullRequestParams,
        GithubPullRequestSummary, GithubRepositoryContentParams, GithubSearchParams,
        MockGithubCodeSearchPort, MockGithubPullRequestPort, MockGithubRepositoryContentPort,
    };

    use super::{
        GithubCodeSearchPort, GithubPullRequestPort, GithubRepositoryContentPort,
        ScopedGithubCodeSearchPort, ScopedGithubPullRequestPort, ScopedGithubRepositoryContentPort,
    };
    use reili_core::source_code::github::GithubScopePolicy;

    fn scope_policy() -> Arc<GithubScopePolicy> {
        Arc::new(GithubScopePolicy::new("acme".to_string()).expect("create scope policy"))
    }

    #[tokio::test]
    async fn search_code_delegates_scope_valid_query_to_inner() {
        let mut inner = MockGithubCodeSearchPort::new();
        inner
            .expect_search_code()
            .once()
            .withf(|params| params.query == "org:AcMe repo:acme/service" && params.limit == 5)
            .return_once(|_| {
                Ok(vec![GithubCodeSearchResultItem {
                    name: "lib.rs".to_string(),
                    path: "src/lib.rs".to_string(),
                    repository_full_name: "acme/service".to_string(),
                    html_url: "https://github.com/acme/service/blob/main/src/lib.rs".to_string(),
                }])
            });

        let port = ScopedGithubCodeSearchPort::new(
            Arc::new(inner) as Arc<dyn GithubCodeSearchPort>,
            scope_policy(),
        );

        let result = port
            .search_code(GithubSearchParams {
                query: "org:AcMe repo:acme/service".to_string(),
                limit: 5,
            })
            .await
            .expect("search code");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].repository_full_name, "acme/service");
    }

    #[tokio::test]
    async fn search_repos_rejects_missing_org_qualifier_before_inner_call() {
        let mut inner = MockGithubCodeSearchPort::new();
        inner.expect_search_repos().times(0);

        let port = ScopedGithubCodeSearchPort::new(
            Arc::new(inner) as Arc<dyn GithubCodeSearchPort>,
            scope_policy(),
        );

        let error = port
            .search_repos(GithubSearchParams {
                query: "language:rust repo:acme/service".to_string(),
                limit: 10,
            })
            .await
            .expect_err("missing org qualifier should fail");

        assert_eq!(error.message, "org qualifier is required. include org:acme");
    }

    #[tokio::test]
    async fn search_issues_rejects_scope_outside_query_before_inner_call() {
        let mut inner = MockGithubCodeSearchPort::new();
        inner.expect_search_issues_and_pull_requests().times(0);

        let port = ScopedGithubCodeSearchPort::new(
            Arc::new(inner) as Arc<dyn GithubCodeSearchPort>,
            scope_policy(),
        );

        let error = port
            .search_issues_and_pull_requests(GithubSearchParams {
                query: "is:pr org:other".to_string(),
                limit: 10,
            })
            .await
            .expect_err("out of scope org qualifier should fail");

        assert_eq!(
            error.message,
            "org qualifier is out of scope. allowed org: acme"
        );
    }

    #[tokio::test]
    async fn repository_content_rejects_scope_outside_owner_before_inner_call() {
        let mut inner = MockGithubRepositoryContentPort::new();
        inner.expect_get_repository_content().times(0);

        let port = ScopedGithubRepositoryContentPort::new(
            Arc::new(inner) as Arc<dyn GithubRepositoryContentPort>,
            scope_policy(),
        );

        let error = port
            .get_repository_content(GithubRepositoryContentParams {
                owner: "other-org".to_string(),
                repo: "service".to_string(),
                path: "src/lib.rs".to_string(),
                r#ref: None,
            })
            .await
            .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[tokio::test]
    async fn pull_request_delegates_scope_valid_owner_to_inner() {
        let mut inner = MockGithubPullRequestPort::new();
        inner
            .expect_get_pull_request()
            .once()
            .withf(|params| {
                params.owner == "AcMe" && params.repo == "service" && params.pull_number == 42
            })
            .return_once(|_| {
                Ok(GithubPullRequestSummary {
                    number: 42,
                    state: "open".to_string(),
                    title: "Fix rollout".to_string(),
                    body: None,
                    user_login: Some("alice".to_string()),
                    created_at: None,
                    updated_at: None,
                    merged_at: None,
                    additions: Some(1),
                    deletions: Some(2),
                    changed_files: Some(1),
                    commits: Some(1),
                    html_url: "https://github.com/acme/service/pull/42".to_string(),
                    base_ref: Some("main".to_string()),
                    head_ref: Some("fix".to_string()),
                })
            });

        let port = ScopedGithubPullRequestPort::new(
            Arc::new(inner) as Arc<dyn GithubPullRequestPort>,
            scope_policy(),
        );

        let result = port
            .get_pull_request(GithubPullRequestParams {
                owner: "AcMe".to_string(),
                repo: "service".to_string(),
                pull_number: 42,
            })
            .await
            .expect("get pull request");

        assert_eq!(result.number, 42);
        assert_eq!(result.state, "open");
    }

    #[tokio::test]
    async fn pull_request_diff_rejects_scope_outside_owner_before_inner_call() {
        let mut inner = MockGithubPullRequestPort::new();
        inner.expect_get_pull_request_diff().times(0);

        let port = ScopedGithubPullRequestPort::new(
            Arc::new(inner) as Arc<dyn GithubPullRequestPort>,
            scope_policy(),
        );

        let error = port
            .get_pull_request_diff(GithubPullRequestParams {
                owner: "other-org".to_string(),
                repo: "service".to_string(),
                pull_number: 42,
            })
            .await
            .expect_err("out of scope owner should fail");

        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }

    #[tokio::test]
    async fn pull_request_diff_delegates_scope_valid_owner_to_inner() {
        let mut inner = MockGithubPullRequestPort::new();
        inner
            .expect_get_pull_request_diff()
            .once()
            .withf(|params| {
                params.owner == "acme" && params.repo == "service" && params.pull_number == 7
            })
            .return_once(|_| {
                Ok(GithubPullRequestDiff {
                    diff: "diff --git a/file b/file".to_string(),
                    html_url: "https://github.com/acme/service/pull/7".to_string(),
                    original_chars: 24,
                    returned_chars: 24,
                    truncated: false,
                })
            });

        let port = ScopedGithubPullRequestPort::new(
            Arc::new(inner) as Arc<dyn GithubPullRequestPort>,
            scope_policy(),
        );

        let result = port
            .get_pull_request_diff(GithubPullRequestParams {
                owner: "acme".to_string(),
                repo: "service".to_string(),
                pull_number: 7,
            })
            .await
            .expect("get pull request diff");

        assert_eq!(result.original_chars, 24);
        assert!(!result.truncated);
    }
}
