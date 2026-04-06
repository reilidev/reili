use std::sync::Arc;

use crate::{
    knowledge::WebSearchPort, messaging::slack::SlackMessageSearchPort, task::TaskCancellation,
};

#[derive(Clone)]
pub struct TaskResources {
    pub slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
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
