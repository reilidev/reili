use reili_core::error::{AgentRunFailedError, PortError, TaskExecutionFailedError};
use reili_core::task::LlmUsageSnapshot;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecuteTaskJobError {
    #[error("{0}")]
    Port(PortError),
    #[error("{0}")]
    AgentRunFailed(AgentRunFailedError),
    #[error("{0}")]
    TaskExecutionFailed(TaskExecutionFailedError),
}

impl ExecuteTaskJobError {
    pub fn is_permanent(&self) -> bool {
        match self {
            Self::AgentRunFailed(value) => value.is_permanent,
            Self::Port(_) | Self::TaskExecutionFailed(_) => false,
        }
    }
}

impl From<PortError> for ExecuteTaskJobError {
    fn from(value: PortError) -> Self {
        Self::Port(value)
    }
}

impl From<AgentRunFailedError> for ExecuteTaskJobError {
    fn from(value: AgentRunFailedError) -> Self {
        Self::AgentRunFailed(value)
    }
}

impl From<TaskExecutionFailedError> for ExecuteTaskJobError {
    fn from(value: TaskExecutionFailedError) -> Self {
        Self::TaskExecutionFailed(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTaskFailureError {
    pub error_message: String,
    pub usage: LlmUsageSnapshot,
}

#[must_use]
pub fn resolve_task_failure_error(error: &ExecuteTaskJobError) -> ResolvedTaskFailureError {
    match error {
        ExecuteTaskJobError::TaskExecutionFailed(value) => ResolvedTaskFailureError {
            error_message: value.cause_message.clone(),
            usage: value.usage.clone(),
        },
        ExecuteTaskJobError::AgentRunFailed(value) => ResolvedTaskFailureError {
            error_message: value.cause_message.clone(),
            usage: value.usage.clone(),
        },
        ExecuteTaskJobError::Port(value) => ResolvedTaskFailureError {
            error_message: value.message.clone(),
            usage: empty_llm_usage_snapshot(),
        },
    }
}

fn empty_llm_usage_snapshot() -> LlmUsageSnapshot {
    LlmUsageSnapshot {
        requests: 0,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use reili_core::error::{AgentRunFailedError, PortError};
    use reili_core::task::LlmUsageSnapshot;

    use super::{ExecuteTaskJobError, resolve_task_failure_error};

    #[test]
    fn resolves_usage_for_task_runner_failure() {
        let error = ExecuteTaskJobError::AgentRunFailed(AgentRunFailedError::new(
            snapshot(2),
            "task_runner failed",
        ));

        let resolved = resolve_task_failure_error(&error);

        assert_eq!(resolved.error_message, "task_runner failed");
        assert_eq!(resolved.usage, snapshot(2));
    }

    #[test]
    fn resolves_usage_for_wrapped_execution_failure() {
        let error = ExecuteTaskJobError::TaskExecutionFailed(
            reili_core::error::TaskExecutionFailedError::new("reply failed", snapshot(3)),
        );

        let resolved = resolve_task_failure_error(&error);

        assert_eq!(resolved.error_message, "reply failed");
        assert_eq!(resolved.usage, snapshot(3));
    }

    #[test]
    fn resolves_usage_for_port_failures() {
        let port_error = ExecuteTaskJobError::Port(PortError::new("slack failed"));

        let port_resolved = resolve_task_failure_error(&port_error);

        assert_eq!(port_resolved.error_message, "slack failed");
        assert_eq!(port_resolved.usage, snapshot(0));
    }

    #[test]
    fn marks_permanent_agent_failures_as_permanent() {
        let error = ExecuteTaskJobError::AgentRunFailed(AgentRunFailedError::new_permanent(
            snapshot(1),
            "mcp failed",
        ));

        assert!(error.is_permanent());
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
