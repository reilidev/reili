use std::fmt::{Display, Formatter};

use thiserror::Error;

use crate::types::LlmUsageSnapshot;

pub const COORDINATOR_RUN_FAILED_CODE: &str = "COORDINATOR_RUN_FAILED";
pub const SYNTHESIZER_RUN_FAILED_CODE: &str = "SYNTHESIZER_RUN_FAILED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Coordinator,
    Synthesizer,
}

impl AgentRole {
    pub fn code(self) -> &'static str {
        match self {
            Self::Coordinator => COORDINATOR_RUN_FAILED_CODE,
            Self::Synthesizer => SYNTHESIZER_RUN_FAILED_CODE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Coordinator => "Coordinator",
            Self::Synthesizer => "Synthesizer",
        }
    }
}

impl Display for AgentRole {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{role} agent run failed: {cause_message}")]
pub struct AgentRunFailedError {
    pub role: AgentRole,
    pub usage: LlmUsageSnapshot,
    pub cause_message: String,
}

impl AgentRunFailedError {
    pub fn code(&self) -> &'static str {
        self.role.code()
    }

    pub fn new(role: AgentRole, usage: LlmUsageSnapshot, cause_message: impl Into<String>) -> Self {
        Self {
            role,
            usage,
            cause_message: cause_message.into(),
        }
    }
}
