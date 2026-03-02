use thiserror::Error;

use crate::types::LlmUsageSnapshot;

pub const SYNTHESIZER_RUN_FAILED_CODE: &str = "SYNTHESIZER_RUN_FAILED";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Synthesizer agent run failed: {cause_message}")]
pub struct SynthesizerRunFailedError {
    pub usage: LlmUsageSnapshot,
    pub cause_message: String,
}

impl SynthesizerRunFailedError {
    pub fn code(&self) -> &'static str {
        SYNTHESIZER_RUN_FAILED_CODE
    }

    pub fn new(usage: LlmUsageSnapshot, cause_message: impl Into<String>) -> Self {
        Self {
            usage,
            cause_message: cause_message.into(),
        }
    }
}
