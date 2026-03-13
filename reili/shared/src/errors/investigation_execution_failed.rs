use thiserror::Error;

use crate::types::{InvestigationLlmTelemetry, LlmUsageSnapshot};

pub const INVESTIGATION_EXECUTION_FAILED_CODE: &str = "INVESTIGATION_EXECUTION_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Investigation execution failed: {cause_message}")]
pub struct InvestigationExecutionFailedError {
    pub cause_message: String,
    pub coordinator_usage: LlmUsageSnapshot,
}

impl InvestigationExecutionFailedError {
    pub fn code(&self) -> &'static str {
        INVESTIGATION_EXECUTION_FAILED_CODE
    }

    pub fn new(cause_message: impl Into<String>, llm_telemetry: InvestigationLlmTelemetry) -> Self {
        Self {
            cause_message: cause_message.into(),
            coordinator_usage: llm_telemetry.coordinator,
        }
    }
}
