use sre_shared::errors::{
    AgentRole, AgentRunFailedError, InvestigationExecutionFailedError, PortError,
};
use sre_shared::types::{InvestigationLlmTelemetry, LlmUsageSnapshot};
use thiserror::Error;

use super::services::create_empty_llm_usage_snapshot;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecuteInvestigationJobError {
    #[error("{0}")]
    Port(PortError),
    #[error("{0}")]
    AgentRunFailed(AgentRunFailedError),
    #[error("{0}")]
    InvestigationExecutionFailed(InvestigationExecutionFailedError),
}

impl From<PortError> for ExecuteInvestigationJobError {
    fn from(value: PortError) -> Self {
        Self::Port(value)
    }
}

impl From<AgentRunFailedError> for ExecuteInvestigationJobError {
    fn from(value: AgentRunFailedError) -> Self {
        Self::AgentRunFailed(value)
    }
}

impl From<InvestigationExecutionFailedError> for ExecuteInvestigationJobError {
    fn from(value: InvestigationExecutionFailedError) -> Self {
        Self::InvestigationExecutionFailed(value)
    }
}

pub struct InvestigationExecutionFailedErrorInput {
    pub cause_message: String,
    pub llm_telemetry: InvestigationLlmTelemetry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInvestigationFailureError {
    pub error_message: String,
    pub coordinator_usage: LlmUsageSnapshot,
    pub synthesizer_usage: LlmUsageSnapshot,
}

#[must_use]
pub fn create_investigation_execution_failed_error(
    input: InvestigationExecutionFailedErrorInput,
) -> InvestigationExecutionFailedError {
    InvestigationExecutionFailedError::new(input.cause_message, input.llm_telemetry)
}

#[must_use]
pub fn resolve_investigation_failure_error(
    error: &ExecuteInvestigationJobError,
) -> ResolvedInvestigationFailureError {
    match error {
        ExecuteInvestigationJobError::InvestigationExecutionFailed(value) => {
            ResolvedInvestigationFailureError {
                error_message: value.cause_message.clone(),
                coordinator_usage: value.coordinator_usage.clone(),
                synthesizer_usage: value.synthesizer_usage.clone(),
            }
        }
        ExecuteInvestigationJobError::AgentRunFailed(value) => match value.role {
            AgentRole::Coordinator => ResolvedInvestigationFailureError {
                error_message: value.cause_message.clone(),
                coordinator_usage: value.usage.clone(),
                synthesizer_usage: create_empty_llm_usage_snapshot(),
            },
            AgentRole::Synthesizer => ResolvedInvestigationFailureError {
                error_message: value.cause_message.clone(),
                coordinator_usage: create_empty_llm_usage_snapshot(),
                synthesizer_usage: value.usage.clone(),
            },
        },
        ExecuteInvestigationJobError::Port(value) => ResolvedInvestigationFailureError {
            error_message: value.message.clone(),
            coordinator_usage: create_empty_llm_usage_snapshot(),
            synthesizer_usage: create_empty_llm_usage_snapshot(),
        },
    }
}

#[cfg(test)]
mod tests {
    use sre_shared::errors::{AgentRole, AgentRunFailedError, PortError};
    use sre_shared::types::{BuildInvestigationLlmTelemetryInput, LlmUsageSnapshot};

    use super::{
        ExecuteInvestigationJobError, InvestigationExecutionFailedErrorInput,
        create_investigation_execution_failed_error, resolve_investigation_failure_error,
    };
    use crate::investigation::services::build_investigation_llm_telemetry;

    #[test]
    fn resolves_usage_for_coordinator_failure() {
        let error = ExecuteInvestigationJobError::AgentRunFailed(AgentRunFailedError::new(
            AgentRole::Coordinator,
            snapshot(2),
            "coordinator failed",
        ));

        let resolved = resolve_investigation_failure_error(&error);

        assert_eq!(resolved.error_message, "coordinator failed");
        assert_eq!(resolved.coordinator_usage, snapshot(2));
        assert_eq!(resolved.synthesizer_usage, snapshot(0));
    }

    #[test]
    fn resolves_usage_for_wrapped_execution_failure() {
        let llm_telemetry =
            build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
                coordinator_usage: snapshot(3),
                synthesizer_usage: snapshot(4),
            });
        let error = ExecuteInvestigationJobError::InvestigationExecutionFailed(
            create_investigation_execution_failed_error(InvestigationExecutionFailedErrorInput {
                cause_message: "reply failed".to_string(),
                llm_telemetry,
            }),
        );

        let resolved = resolve_investigation_failure_error(&error);

        assert_eq!(resolved.error_message, "reply failed");
        assert_eq!(resolved.coordinator_usage, snapshot(3));
        assert_eq!(resolved.synthesizer_usage, snapshot(4));
    }

    #[test]
    fn resolves_usage_for_port_and_synthesizer_failures() {
        let port_error = ExecuteInvestigationJobError::Port(PortError::new("slack failed"));
        let synth_error = ExecuteInvestigationJobError::AgentRunFailed(AgentRunFailedError::new(
            AgentRole::Synthesizer,
            snapshot(5),
            "synth failed",
        ));

        let port_resolved = resolve_investigation_failure_error(&port_error);
        let synth_resolved = resolve_investigation_failure_error(&synth_error);

        assert_eq!(port_resolved.error_message, "slack failed");
        assert_eq!(port_resolved.coordinator_usage, snapshot(0));
        assert_eq!(port_resolved.synthesizer_usage, snapshot(0));

        assert_eq!(synth_resolved.error_message, "synth failed");
        assert_eq!(synth_resolved.coordinator_usage, snapshot(0));
        assert_eq!(synth_resolved.synthesizer_usage, snapshot(5));
    }

    fn snapshot(requests: u32) -> LlmUsageSnapshot {
        LlmUsageSnapshot {
            requests,
            input_tokens: requests as u64,
            output_tokens: requests as u64,
            total_tokens: requests as u64 * 2,
        }
    }
}
