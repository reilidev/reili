use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackTaskControlMessagePort, SlackTaskControlState, UpdateTaskControlMessageInput,
};
use reili_core::queue::{CancelJobInput, CancelJobResult, TaskJobQueuePort};
use reili_core::task::TaskJob;

use crate::task::services::{
    InFlightJobCancellationInfo, InFlightJobRegistry, RequestCancelInFlightJobResult,
};
use crate::task::{TaskLogger, string_log_meta};

pub struct CancelTaskUseCaseDeps {
    pub job_queue: Arc<TaskJobQueuePort>,
    pub in_flight_job_registry: InFlightJobRegistry,
    pub slack_task_control_message_port: Arc<dyn SlackTaskControlMessagePort>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct CancelTaskUseCase {
    deps: CancelTaskUseCaseDeps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelTaskInput {
    pub job_id: String,
    pub requested_by_user_id: String,
    pub channel: String,
    pub thread_ts: String,
    pub message_ts: String,
}

impl CancelTaskUseCase {
    pub fn new(deps: CancelTaskUseCaseDeps) -> Self {
        Self { deps }
    }

    pub async fn execute(&self, input: CancelTaskInput) -> Result<(), PortError> {
        match self
            .deps
            .job_queue
            .cancel(CancelJobInput {
                job_id: input.job_id.clone(),
            })
            .await?
        {
            CancelJobResult::Cancelled(job) => {
                self.deps.logger.info(
                    "job_cancelled",
                    string_log_meta([
                        ("jobId", job.job_id.clone()),
                        ("requestedBy", input.requested_by_user_id.clone()),
                        ("channel", job.payload.message.channel.clone()),
                        (
                            "threadTs",
                            job.payload.message.thread_ts_or_ts().to_string(),
                        ),
                        ("messageTs", job.payload.control_message_ts.clone()),
                    ]),
                );
                self.update_job_control_message(
                    &job,
                    SlackTaskControlState::Cancelled {
                        cancelled_by_user_id: input.requested_by_user_id,
                    },
                )
                .await;
                Ok(())
            }
            CancelJobResult::AlreadyClaimed => {
                let requested_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
                let handle = match self
                    .deps
                    .in_flight_job_registry
                    .request_cancel(
                        &input.job_id,
                        input.requested_by_user_id.clone(),
                        requested_at,
                    )
                    .await
                {
                    RequestCancelInFlightJobResult::Accepted(handle)
                    | RequestCancelInFlightJobResult::AlreadyRequested(handle) => handle,
                };

                self.log_cancel_requested(&handle, &input);
                self.update_control_message(
                    input.channel,
                    input.thread_ts,
                    input.message_ts,
                    input.job_id,
                    SlackTaskControlState::CancellationRequested {
                        requested_by_user_id: input.requested_by_user_id,
                    },
                )
                .await;
                Ok(())
            }
            CancelJobResult::NotFound => {
                self.deps.logger.info(
                    "job_cancel_ignored",
                    string_log_meta([
                        ("jobId", input.job_id),
                        ("requestedBy", input.requested_by_user_id),
                        ("channel", input.channel),
                        ("threadTs", input.thread_ts),
                        ("messageTs", input.message_ts),
                        ("reason", "not_found".to_string()),
                    ]),
                );
                Ok(())
            }
        }
    }

    async fn update_job_control_message(&self, job: &TaskJob, state: SlackTaskControlState) {
        self.update_control_message(
            job.payload.message.channel.clone(),
            job.payload.message.thread_ts_or_ts().to_string(),
            job.payload.control_message_ts.clone(),
            job.job_id.clone(),
            state,
        )
        .await;
    }

    async fn update_control_message(
        &self,
        channel: String,
        thread_ts: String,
        message_ts: String,
        job_id: String,
        state: SlackTaskControlState,
    ) {
        if let Err(error) = self
            .deps
            .slack_task_control_message_port
            .update_task_control_message(UpdateTaskControlMessageInput {
                channel: channel.clone(),
                thread_ts: thread_ts.clone(),
                message_ts: message_ts.clone(),
                job_id: job_id.clone(),
                state,
            })
            .await
        {
            self.deps.logger.warn(
                "task_control_message_update_failed",
                string_log_meta([
                    ("jobId", job_id),
                    ("channel", channel),
                    ("threadTs", thread_ts),
                    ("messageTs", message_ts),
                    ("error", error.message),
                ]),
            );
        }
    }

    fn log_cancel_requested(
        &self,
        cancellation_info: &InFlightJobCancellationInfo,
        input: &CancelTaskInput,
    ) {
        self.deps.logger.info(
            "job_cancel_requested",
            string_log_meta([
                ("jobId", cancellation_info.job_id.clone()),
                (
                    "requestedBy",
                    cancellation_info
                        .cancel_requested_by_user_id
                        .clone()
                        .unwrap_or_else(|| input.requested_by_user_id.clone()),
                ),
                ("channel", input.channel.clone()),
                ("threadTs", input.thread_ts.clone()),
                ("messageTs", input.message_ts.clone()),
            ]),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::messaging::slack::{
        MockSlackTaskControlMessagePort, SlackTaskControlMessagePort, SlackTaskControlState,
        UpdateTaskControlMessageInput,
    };
    use reili_core::queue::{CancelJobResult, MockJobQueuePort, TaskJobQueuePort};
    use reili_core::task::TaskJob;

    use super::{CancelTaskInput, CancelTaskUseCase, CancelTaskUseCaseDeps};
    use crate::task::services::InFlightJobRegistry;
    use crate::task::{LogEntry, TaskLogger};

    #[derive(Default)]
    struct NoopLogger;

    impl TaskLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    #[tokio::test]
    async fn queued_task_is_cancelled_immediately() {
        let mut job_queue = MockJobQueuePort::<TaskJob>::new();
        job_queue
            .expect_cancel()
            .times(1)
            .returning(|_| Ok(CancelJobResult::Cancelled(sample_job())));

        let update_calls = Arc::new(Mutex::new(Vec::new()));
        let mut control_port = MockSlackTaskControlMessagePort::new();
        let update_calls_ref = Arc::clone(&update_calls);
        control_port
            .expect_update_task_control_message()
            .times(1)
            .returning(move |input: UpdateTaskControlMessageInput| {
                update_calls_ref
                    .lock()
                    .expect("lock update calls")
                    .push(input);
                Ok(())
            });

        let use_case = CancelTaskUseCase::new(CancelTaskUseCaseDeps {
            job_queue: Arc::new(job_queue) as Arc<TaskJobQueuePort>,
            in_flight_job_registry: InFlightJobRegistry::new(),
            slack_task_control_message_port: Arc::new(control_port)
                as Arc<dyn SlackTaskControlMessagePort>,
            logger: Arc::new(NoopLogger),
        });

        use_case
            .execute(CancelTaskInput {
                job_id: "job-1".to_string(),
                requested_by_user_id: "U002".to_string(),
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                message_ts: "1710000000.000002".to_string(),
            })
            .await
            .expect("cancel task");

        assert_eq!(update_calls.lock().expect("lock update calls").len(), 1);
        assert!(matches!(
            update_calls.lock().expect("lock update calls")[0].state,
            SlackTaskControlState::Cancelled { .. }
        ));
    }

    fn sample_job() -> TaskJob {
        TaskJob {
            job_id: "job-1".to_string(),
            received_at: "2026-03-31T00:00:00.000Z".to_string(),
            payload: reili_core::task::TaskJobPayload {
                slack_event_id: "evt-1".to_string(),
                message: reili_core::messaging::slack::SlackMessage {
                    slack_event_id: "evt-1".to_string(),
                    team_id: Some("T001".to_string()),
                    action_token: None,
                    trigger: reili_core::messaging::slack::SlackTriggerType::AppMention,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    text: "please investigate".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    ts: "1710000000.000001".to_string(),
                    thread_ts: None,
                },
                control_message_ts: "1710000000.000002".to_string(),
            },
            retry_count: 0,
        }
    }
}
