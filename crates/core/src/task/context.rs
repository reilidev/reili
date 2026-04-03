use std::sync::Arc;

use crate::{
    knowledge::WebSearchPort,
    source_code::github::{
        GithubCodeSearchPort, GithubPullRequestPort, GithubRepositoryContentPort,
    },
    task::TaskCancellation,
};

#[derive(Clone)]
pub struct TaskResources {
    pub github_code_search_port: Arc<dyn GithubCodeSearchPort>,
    pub github_repository_content_port: Arc<dyn GithubRepositoryContentPort>,
    pub github_pull_request_port: Arc<dyn GithubPullRequestPort>,
    pub web_search_port: Arc<dyn WebSearchPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRuntime {
    pub started_at_iso: String,
    pub channel: String,
    pub thread_ts: String,
    pub retry_count: u32,
}

pub struct TaskContext {
    pub resources: TaskResources,
    pub runtime: TaskRuntime,
    pub cancellation: TaskCancellation,
}
