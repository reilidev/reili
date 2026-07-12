use std::sync::Arc;

use crate::{
    knowledge::WebSearchPort,
    messaging::slack::{SlackCanvasMemoryPort, SlackFileDownloadPort, SlackMessageSearchPort},
    task::TaskCancellation,
};

#[derive(Clone)]
pub struct TaskResources {
    pub slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
    pub slack_file_download_port: Arc<dyn SlackFileDownloadPort>,
    pub web_search_port: Arc<dyn WebSearchPort>,
    /// Optional channel memory backed by a Slack Canvas. `None` disables the memory
    /// feature: the loader yields no context and the `save_memory` tool is not registered.
    pub canvas_memory_port: Option<Arc<dyn SlackCanvasMemoryPort>>,
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
