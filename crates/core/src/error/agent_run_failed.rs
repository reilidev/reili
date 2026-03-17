use thiserror::Error;

use crate::investigation::LlmUsageSnapshot;

pub const INVESTIGATION_LEAD_RUN_FAILED_CODE: &str = "INVESTIGATION_LEAD_RUN_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("InvestigationLead run failed: {cause_message}")]
pub struct AgentRunFailedError {
    pub usage: LlmUsageSnapshot,
    pub cause_message: String,
    pub is_permanent: bool,
}

impl AgentRunFailedError {
    pub fn code(&self) -> &'static str {
        INVESTIGATION_LEAD_RUN_FAILED_CODE
    }

    pub fn new(usage: LlmUsageSnapshot, cause_message: impl Into<String>) -> Self {
        Self {
            usage,
            cause_message: cause_message.into(),
            is_permanent: false,
        }
    }

    pub fn new_permanent(usage: LlmUsageSnapshot, cause_message: impl Into<String>) -> Self {
        Self {
            usage,
            cause_message: cause_message.into(),
            is_permanent: true,
        }
    }
}
