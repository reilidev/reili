#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMemoryItem {
    pub source: TaskMemorySource,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMemorySource {
    pub channel_id: String,
    pub message_ts: String,
    pub thread_ts: Option<String>,
    pub permalink: Option<String>,
}
