use std::sync::Arc;

use crate::logger::Logger;
use async_trait::async_trait;

use crate::error::AgentRunFailedError;

use super::{LlmUsageSnapshot, TaskContext, TaskProgressEventPort, TaskRequest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmExecutionMetadata {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRunReport {
    pub result_text: String,
    pub usage: LlmUsageSnapshot,
    pub execution: LlmExecutionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRunOutcome {
    Succeeded(TaskRunReport),
    Cancelled,
}

pub struct RunTaskInput {
    pub request: TaskRequest,
    pub context: TaskContext,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub logger: Arc<dyn Logger>,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait TaskRunnerPort: Send + Sync {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError>;
}
