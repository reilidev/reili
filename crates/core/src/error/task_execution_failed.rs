use thiserror::Error;

use crate::task::LlmUsageSnapshot;

pub const TASK_EXECUTION_FAILED_CODE: &str = "TASK_EXECUTION_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Task execution failed: {cause_message}")]
pub struct TaskExecutionFailedError {
    pub cause_message: String,
    pub usage: LlmUsageSnapshot,
}

impl TaskExecutionFailedError {
    pub fn code(&self) -> &'static str {
        TASK_EXECUTION_FAILED_CODE
    }

    pub fn new(cause_message: impl Into<String>, usage: LlmUsageSnapshot) -> Self {
        Self {
            cause_message: cause_message.into(),
            usage,
        }
    }
}
