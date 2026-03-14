use reili_core::error::{AgentRunFailedError, InvestigationExecutionFailedError, PortError};
use reili_core::investigation::LlmUsageSnapshot;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInvestigationFailureError {
    pub error_message: String,
    pub usage: LlmUsageSnapshot,
}

#[must_use]
pub fn resolve_investigation_failure_error(
    error: &ExecuteInvestigationJobError,
) -> ResolvedInvestigationFailureError {
    match error {
        ExecuteInvestigationJobError::InvestigationExecutionFailed(value) => {
            ResolvedInvestigationFailureError {
                error_message: value.cause_message.clone(),
                usage: value.usage.clone(),
            }
        }
        ExecuteInvestigationJobError::AgentRunFailed(value) => ResolvedInvestigationFailureError {
            error_message: value.cause_message.clone(),
            usage: value.usage.clone(),
        },
        ExecuteInvestigationJobError::Port(value) => ResolvedInvestigationFailureError {
            error_message: value.message.clone(),
            usage: create_empty_llm_usage_snapshot(),
        },
    }
}

#[cfg(test)]
mod tests {
    use reili_core::error::{AgentRunFailedError, PortError};
    use reili_core::investigation::{BuildInvestigationLlmTelemetryInput, LlmUsageSnapshot};

    use super::{ExecuteInvestigationJobError, resolve_investigation_failure_error};
    use crate::investigation::services::build_investigation_llm_telemetry;

    #[test]
    fn resolves_usage_for_coordinator_failure() {
        let error = ExecuteInvestigationJobError::AgentRunFailed(AgentRunFailedError::new(
            snapshot(2),
            "coordinator failed",
        ));

        let resolved = resolve_investigation_failure_error(&error);

        assert_eq!(resolved.error_message, "coordinator failed");
        assert_eq!(resolved.usage, snapshot(2));
    }

    #[test]
    fn resolves_usage_for_wrapped_execution_failure() {
        let llm_telemetry =
            build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
                usage: snapshot(3),
            });
        let error = ExecuteInvestigationJobError::InvestigationExecutionFailed(
            reili_core::error::InvestigationExecutionFailedError::new(
                "reply failed",
                llm_telemetry,
            ),
        );

        let resolved = resolve_investigation_failure_error(&error);

        assert_eq!(resolved.error_message, "reply failed");
        assert_eq!(resolved.usage, snapshot(3));
    }

    #[test]
    fn resolves_usage_for_port_failures() {
        let port_error = ExecuteInvestigationJobError::Port(PortError::new("slack failed"));

        let port_resolved = resolve_investigation_failure_error(&port_error);

        assert_eq!(port_resolved.error_message, "slack failed");
        assert_eq!(port_resolved.usage, snapshot(0));
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
