use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

pub const TASK_RUNNER_PROGRESS_OWNER_ID: &str = "task_runner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskProgressEvent {
    ProgressSummaryCreated { title: String, summary: String },
    ToolCallStarted { task_id: String, title: String },
    ToolCallCompleted { task_id: String, title: String },
    MessageOutputCreated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEventInput {
    pub owner_id: String,
    pub event: TaskProgressEvent,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait TaskProgressEventPort: Send + Sync {
    async fn publish(&self, input: TaskProgressEventInput) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::{TaskProgressEvent, TaskProgressEventInput};

    #[test]
    fn serializes_and_deserializes_progress_event() {
        let value = TaskProgressEventInput {
            owner_id: "task_runner".to_string(),
            event: TaskProgressEvent::ToolCallStarted {
                task_id: "task-1".to_string(),
                title: "Query logs".to_string(),
            },
        };

        let json = serde_json::to_string(&value).expect("serialize progress event");
        let restored: TaskProgressEventInput =
            serde_json::from_str(&json).expect("deserialize progress event");

        assert_eq!(restored, value);
    }
}
