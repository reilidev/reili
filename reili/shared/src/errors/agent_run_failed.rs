use thiserror::Error;

use crate::types::LlmUsageSnapshot;

pub const COORDINATOR_RUN_FAILED_CODE: &str = "COORDINATOR_RUN_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Coordinator run failed: {cause_message}")]
pub struct AgentRunFailedError {
    pub usage: LlmUsageSnapshot,
    pub cause_message: String,
}

impl AgentRunFailedError {
    pub fn code(&self) -> &'static str {
        COORDINATOR_RUN_FAILED_CODE
    }

    pub fn new(usage: LlmUsageSnapshot, cause_message: impl Into<String>) -> Self {
        Self {
            usage,
            cause_message: cause_message.into(),
        }
    }
}
