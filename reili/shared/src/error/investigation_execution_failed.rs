use thiserror::Error;

use crate::investigation::{InvestigationLlmTelemetry, LlmUsageSnapshot};

pub const INVESTIGATION_EXECUTION_FAILED_CODE: &str = "INVESTIGATION_EXECUTION_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Investigation execution failed: {cause_message}")]
pub struct InvestigationExecutionFailedError {
    pub cause_message: String,
    pub usage: LlmUsageSnapshot,
}

impl InvestigationExecutionFailedError {
    pub fn code(&self) -> &'static str {
        INVESTIGATION_EXECUTION_FAILED_CODE
    }

    pub fn new(cause_message: impl Into<String>, llm_telemetry: InvestigationLlmTelemetry) -> Self {
        Self {
            cause_message: cause_message.into(),
            usage: llm_telemetry.coordinator,
        }
    }
}
