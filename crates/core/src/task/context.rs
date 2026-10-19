use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::{
    knowledge::WebSearchPort,
    messaging::slack::{SlackCanvasMemoryPort, SlackFileDownloadPort, SlackMessageSearchPort},
    task::TaskCancellation,
};

#[derive(Clone)]
pub struct TaskResources {
    pub slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
    pub slack_file_download_port: Arc<dyn SlackFileDownloadPort>,
    pub web_search_port: Option<Arc<dyn WebSearchPort>>,
    /// Optional channel memory backed by a Slack Canvas. `None` disables the memory
    /// feature: the loader yields no context and the `save_memory` tool is not registered.
    pub canvas_memory_port: Option<Arc<dyn SlackCanvasMemoryPort>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRuntime {
    pub started_at_iso: String,
    pub channel: String,
    pub thread_ts: String,
    pub retry_count: u32,
}

pub struct ObjectiveCycleRuntimeInput<'a> {
    pub objective_id: &'a str,
    pub run_id: &'a str,
    pub started_at: DateTime<Utc>,
}

impl TaskRuntime {
    /// An objective cycle has no Slack thread. Its objective and run take the place of the
    /// channel and thread so logs and progress still identify the run.
    #[must_use]
    pub fn for_objective_cycle(input: ObjectiveCycleRuntimeInput<'_>) -> Self {
        Self {
            started_at_iso: input.started_at.to_rfc3339(),
            channel: format!("objective:{}", input.objective_id),
            thread_ts: input.run_id.to_string(),
            retry_count: 0,
        }
    }
}

pub struct TaskContext {
    pub resources: TaskResources,
    pub runtime: TaskRuntime,
    pub cancellation: TaskCancellation,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{ObjectiveCycleRuntimeInput, TaskRuntime};

    #[test]
    fn objective_cycle_runtime_identifies_the_objective_and_run() {
        let started_at = Utc
            .with_ymd_and_hms(2026, 10, 19, 0, 0, 0)
            .single()
            .expect("time");

        let runtime = TaskRuntime::for_objective_cycle(ObjectiveCycleRuntimeInput {
            objective_id: "checkout",
            run_id: "run-1",
            started_at,
        });

        assert_eq!(
            runtime,
            TaskRuntime {
                started_at_iso: "2026-10-19T00:00:00+00:00".to_string(),
                channel: "objective:checkout".to_string(),
                thread_ts: "run-1".to_string(),
                retry_count: 0,
            }
        );
    }
}
