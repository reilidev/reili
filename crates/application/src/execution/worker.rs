use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackTaskControlMessagePort, SlackTaskControlState, SlackThreadReplyInput,
    SlackThreadReplyPort, UpdateTaskControlMessageInput,
};
use reili_core::queue::{CompleteJobInput, FailJobInput, JobFailStatus, TaskJobQueuePort};
use reili_core::task::{TaskCancellation, TaskJob};
use tokio::task::spawn;
use tokio::time::sleep;

use crate::{TaskLogger, string_log_meta};

use super::in_flight_registry::{
    AttachCancellationResult, InFlightJobCancellationInfo, InFlightJobRegistry,
};
use super::{ExecuteTaskJobInput, TaskExecutionDeps, TaskExecutionOutcome, execute_task_job};

const IDLE_WAIT_MS: u64 = 150;

pub struct StartTaskWorkerRunnerUseCaseDeps {
    pub job_queue: Arc<TaskJobQueuePort>,
    pub in_flight_job_registry: InFlightJobRegistry,
    pub slack_task_control_message_port: Arc<dyn SlackTaskControlMessagePort>,
    pub task_execution_deps: TaskExecutionDeps,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
}

pub struct StartTaskWorkerRunnerUseCase {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    is_running: Arc<AtomicBool>,
}

impl StartTaskWorkerRunnerUseCase {
    pub fn new(deps: StartTaskWorkerRunnerUseCaseDeps) -> Self {
        Self {
            deps: Arc::new(deps),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) {
        if self.is_running.swap(true, Ordering::SeqCst) {
            return;
        }

        if self.deps.worker_concurrency == 0 {
            self.is_running.store(false, Ordering::SeqCst);
            return;
        }

        for worker_index in 0..self.deps.worker_concurrency {
            spawn(run_task_worker_loop(WorkerLoopInput {
                deps: Arc::clone(&self.deps),
                is_running: Arc::clone(&self.is_running),
                worker_index,
            }));
        }
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

struct WorkerLoopInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    is_running: Arc<AtomicBool>,
    worker_index: u32,
}

async fn run_task_worker_loop(input: WorkerLoopInput) {
    while input.is_running.load(Ordering::SeqCst) {
        run_worker_iteration(RunWorkerIterationInput {
            deps: Arc::clone(&input.deps),
            worker_index: input.worker_index,
        })
        .await;
    }
}

struct RunWorkerIterationInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    worker_index: u32,
}

async fn run_worker_iteration(input: RunWorkerIterationInput) {
    match input.deps.job_queue.claim().await {
        Ok(Some(job)) => {
            process_claimed_job(ProcessClaimedJobInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
                job,
            })
            .await;
        }
        Ok(None) => {
            sleep(Duration::from_millis(IDLE_WAIT_MS)).await;
        }
        Err(error) => {
            input.deps.task_execution_deps.logger.error(
                "Failed to claim worker job",
                string_log_meta([
                    ("workerIndex", input.worker_index.to_string()),
                    ("error", error.message),
                ]),
            );
            sleep(Duration::from_millis(IDLE_WAIT_MS)).await;
        }
    }
}

struct ProcessClaimedJobInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    worker_index: u32,
    job: TaskJob,
}

async fn process_claimed_job(input: ProcessClaimedJobInput) {
    let started_at = Instant::now();
    let _ = input
        .deps
        .in_flight_job_registry
        .register_claimed(input.job.job_id.clone())
        .await;
    let task_cancellation = TaskCancellation::new();
    let attach_result = input
        .deps
        .in_flight_job_registry
        .attach_cancellation(&input.job.job_id, task_cancellation.clone())
        .await;

    match attach_result {
        AttachCancellationResult::Running(_) => {
            update_task_control_message(
                Arc::clone(&input.deps.slack_task_control_message_port),
                Arc::clone(&input.deps.task_execution_deps.logger),
                &input.job,
                SlackTaskControlState::Running,
            )
            .await;
        }
        AttachCancellationResult::CancelRequested(handle) => {
            complete_cancelled_claimed_job(CompleteCancelledClaimedJobInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
                job: input.job,
                started_at,
                handle: Some(handle),
            })
            .await;
            return;
        }
    }

    match execute_task_job(ExecuteTaskJobInput {
        job_id: input.job.job_id.clone(),
        retry_count: input.job.retry_count,
        payload: input.job.payload.clone(),
        task_cancellation,
        deps: input.deps.task_execution_deps.clone(),
    })
    .await
    {
        Ok(TaskExecutionOutcome::Succeeded) => {
            match input
                .deps
                .job_queue
                .complete(CompleteJobInput {
                    job_id: input.job.job_id.clone(),
                })
                .await
            {
                Ok(()) => {
                    let _ = input
                        .deps
                        .in_flight_job_registry
                        .remove(&input.job.job_id)
                        .await;
                    update_task_control_message(
                        Arc::clone(&input.deps.slack_task_control_message_port),
                        Arc::clone(&input.deps.task_execution_deps.logger),
                        &input.job,
                        SlackTaskControlState::Completed,
                    )
                    .await;
                    let queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
                        deps: Arc::clone(&input.deps),
                        worker_index: input.worker_index,
                    })
                    .await;

                    input.deps.task_execution_deps.logger.info(
                        "Completed worker job",
                        string_log_meta([
                            ("workerIndex", input.worker_index.to_string()),
                            ("slackEventId", input.job.payload.slack_event_id.clone()),
                            ("jobId", input.job.job_id),
                            ("channel", input.job.payload.message.channel.clone()),
                            (
                                "threadTs",
                                input.job.payload.message.thread_ts_or_ts().to_string(),
                            ),
                            (
                                "attempt",
                                input.job.retry_count.saturating_add(1).to_string(),
                            ),
                            (
                                "worker_job_duration_ms",
                                started_at.elapsed().as_millis().to_string(),
                            ),
                            ("worker_queue_depth", queue_depth),
                        ]),
                    );
                }
                Err(error) => {
                    handle_failed_claimed_job(HandleFailedClaimedJobInput {
                        deps: Arc::clone(&input.deps),
                        worker_index: input.worker_index,
                        job: input.job,
                        started_at,
                        error_message: error.message,
                        failure_disposition: FailureDisposition::Retryable,
                    })
                    .await;
                }
            }
        }
        Ok(TaskExecutionOutcome::Cancelled) => {
            complete_cancelled_claimed_job(CompleteCancelledClaimedJobInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
                job: input.job,
                started_at,
                handle: None,
            })
            .await;
        }
        Err(error) => {
            handle_failed_claimed_job(HandleFailedClaimedJobInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
                job: input.job,
                started_at,
                error_message: error.to_string(),
                failure_disposition: if error.is_permanent() {
                    FailureDisposition::Permanent
                } else {
                    FailureDisposition::Retryable
                },
            })
            .await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureDisposition {
    Retryable,
    Permanent,
}

struct HandleFailedClaimedJobInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    worker_index: u32,
    job: TaskJob,
    started_at: Instant,
    error_message: String,
    failure_disposition: FailureDisposition,
}

struct CompleteCancelledClaimedJobInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    worker_index: u32,
    job: TaskJob,
    started_at: Instant,
    handle: Option<InFlightJobCancellationInfo>,
}

async fn complete_cancelled_claimed_job(input: CompleteCancelledClaimedJobInput) {
    match input
        .deps
        .job_queue
        .complete(CompleteJobInput {
            job_id: input.job.job_id.clone(),
        })
        .await
    {
        Ok(()) => {
            let handle = input
                .deps
                .in_flight_job_registry
                .remove(&input.job.job_id)
                .await
                .or(input.handle);

            if let Some(handle) = handle.as_ref() {
                update_task_control_message(
                    Arc::clone(&input.deps.slack_task_control_message_port),
                    Arc::clone(&input.deps.task_execution_deps.logger),
                    &input.job,
                    SlackTaskControlState::Cancelled {
                        cancelled_by_user_id: handle
                            .cancel_requested_by_user_id
                            .clone()
                            .unwrap_or_default(),
                    },
                )
                .await;
            }

            let queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
            })
            .await;

            input.deps.task_execution_deps.logger.info(
                "Cancelled worker job",
                string_log_meta([
                    ("workerIndex", input.worker_index.to_string()),
                    ("slackEventId", input.job.payload.slack_event_id),
                    ("jobId", input.job.job_id),
                    ("channel", input.job.payload.message.channel.clone()),
                    (
                        "threadTs",
                        input.job.payload.message.thread_ts_or_ts().to_string(),
                    ),
                    (
                        "attempt",
                        input.job.retry_count.saturating_add(1).to_string(),
                    ),
                    (
                        "worker_job_duration_ms",
                        input.started_at.elapsed().as_millis().to_string(),
                    ),
                    ("worker_queue_depth", queue_depth),
                ]),
            );
        }
        Err(error) => {
            input.deps.task_execution_deps.logger.error(
                "Failed to complete cancelled worker job",
                string_log_meta([
                    ("workerIndex", input.worker_index.to_string()),
                    ("jobId", input.job.job_id),
                    ("error", error.message),
                ]),
            );
        }
    }
}

async fn handle_failed_claimed_job(input: HandleFailedClaimedJobInput) {
    let effective_max_retry = match input.failure_disposition {
        FailureDisposition::Permanent => input.job.retry_count,
        FailureDisposition::Retryable => input.deps.job_max_retry,
    };
    let fail_result = input
        .deps
        .job_queue
        .fail(FailJobInput {
            job_id: input.job.job_id.clone(),
            reason: input.error_message.clone(),
            max_retry: effective_max_retry,
            backoff_ms: input.deps.job_backoff_ms,
        })
        .await;

    let fail_result = match fail_result {
        Ok(value) => value,
        Err(queue_fail_error) => {
            input.deps.task_execution_deps.logger.error(
                "Failed worker job",
                string_log_meta([
                    ("workerIndex", input.worker_index.to_string()),
                    ("slackEventId", input.job.payload.slack_event_id.clone()),
                    ("jobId", input.job.job_id),
                    ("channel", input.job.payload.message.channel.clone()),
                    (
                        "threadTs",
                        input.job.payload.message.thread_ts_or_ts().to_string(),
                    ),
                    (
                        "attempt",
                        input.job.retry_count.saturating_add(1).to_string(),
                    ),
                    (
                        "worker_job_duration_ms",
                        input.started_at.elapsed().as_millis().to_string(),
                    ),
                    ("status", "queue_fail_error".to_string()),
                    ("error", queue_fail_error.message),
                ]),
            );
            return;
        }
    };

    match fail_result.status {
        JobFailStatus::Requeued => {
            let _ = input
                .deps
                .in_flight_job_registry
                .remove(&input.job.job_id)
                .await;
        }
        JobFailStatus::DeadLetter => {
            let _ = input
                .deps
                .in_flight_job_registry
                .remove(&input.job.job_id)
                .await;
            update_task_control_message(
                Arc::clone(&input.deps.slack_task_control_message_port),
                Arc::clone(&input.deps.task_execution_deps.logger),
                &input.job,
                SlackTaskControlState::Failed,
            )
            .await;
        }
    }

    let queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
        deps: Arc::clone(&input.deps),
        worker_index: input.worker_index,
    })
    .await;

    input.deps.task_execution_deps.logger.error(
        "Failed worker job",
        string_log_meta([
            ("workerIndex", input.worker_index.to_string()),
            ("slackEventId", input.job.payload.slack_event_id.clone()),
            ("jobId", input.job.job_id.clone()),
            ("channel", input.job.payload.message.channel.clone()),
            (
                "threadTs",
                input.job.payload.message.thread_ts_or_ts().to_string(),
            ),
            (
                "attempt",
                input.job.retry_count.saturating_add(1).to_string(),
            ),
            (
                "worker_job_duration_ms",
                input.started_at.elapsed().as_millis().to_string(),
            ),
            ("worker_queue_depth", queue_depth),
            ("worker_job_failure_total", "1".to_string()),
            ("status", job_fail_status_to_string(&fail_result.status)),
            ("error", input.error_message.clone()),
        ]),
    );

    if fail_result.status == JobFailStatus::DeadLetter
        && let Err(dead_letter_error) =
            post_dead_letter_failure_message(PostDeadLetterFailureMessageInput {
                slack_reply_port: Arc::clone(&input.deps.task_execution_deps.slack_reply_port),
                job: fail_result.job.clone(),
                error_message: input.error_message.clone(),
                exhausted_retries: matches!(
                    input.failure_disposition,
                    FailureDisposition::Retryable
                ),
            })
            .await
    {
        input.deps.task_execution_deps.logger.error(
            "Failed dead-letter notification",
            string_log_meta([
                ("slackEventId", fail_result.job.payload.slack_event_id),
                ("jobId", fail_result.job.job_id),
                ("channel", fail_result.job.payload.message.channel.clone()),
                (
                    "threadTs",
                    fail_result
                        .job
                        .payload
                        .message
                        .thread_ts_or_ts()
                        .to_string(),
                ),
                ("error", dead_letter_error.message),
            ]),
        );
    }
}

async fn update_task_control_message(
    slack_task_control_message_port: Arc<dyn SlackTaskControlMessagePort>,
    logger: Arc<dyn TaskLogger>,
    job: &TaskJob,
    state: SlackTaskControlState,
) {
    if let Err(error) = slack_task_control_message_port
        .update_task_control_message(UpdateTaskControlMessageInput {
            channel: job.payload.message.channel.clone(),
            thread_ts: job.payload.message.thread_ts_or_ts().to_string(),
            message_ts: job.payload.control_message_ts.clone(),
            job_id: job.job_id.clone(),
            state,
        })
        .await
    {
        logger.warn(
            "task_control_message_update_failed",
            string_log_meta([
                ("jobId", job.job_id.clone()),
                ("channel", job.payload.message.channel.clone()),
                (
                    "threadTs",
                    job.payload.message.thread_ts_or_ts().to_string(),
                ),
                ("messageTs", job.payload.control_message_ts.clone()),
                ("error", error.message),
            ]),
        );
    }
}

struct ReadWorkerQueueDepthInput {
    deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
    worker_index: u32,
}

async fn read_worker_queue_depth(input: ReadWorkerQueueDepthInput) -> String {
    match input.deps.job_queue.get_depth().await {
        Ok(value) => value.to_string(),
        Err(error) => {
            input.deps.task_execution_deps.logger.error(
                "Failed to read worker queue depth",
                string_log_meta([
                    ("workerIndex", input.worker_index.to_string()),
                    ("error", error.message),
                ]),
            );
            "unknown".to_string()
        }
    }
}

struct PostDeadLetterFailureMessageInput {
    slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    job: TaskJob,
    error_message: String,
    exhausted_retries: bool,
}

async fn post_dead_letter_failure_message(
    input: PostDeadLetterFailureMessageInput,
) -> Result<(), PortError> {
    input
        .slack_reply_port
        .post_thread_reply(SlackThreadReplyInput {
            channel: input.job.payload.message.channel.clone(),
            thread_ts: input.job.payload.message.thread_ts_or_ts().to_string(),
            text: if input.exhausted_retries {
                format!("Task failed after retries: {}", input.error_message)
            } else {
                format!("Task failed: {}", input.error_message)
            },
        })
        .await
}

fn job_fail_status_to_string(value: &JobFailStatus) -> String {
    match value {
        JobFailStatus::Requeued => "requeued".to_string(),
        JobFailStatus::DeadLetter => "dead_letter".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Arc, JobFailStatus, PortError, ProcessClaimedJobInput, SlackThreadReplyInput,
        SlackThreadReplyPort, StartTaskWorkerRunnerUseCaseDeps, TaskExecutionDeps, TaskJob,
        TaskJobQueuePort, handle_failed_claimed_job, process_claimed_job,
    };
    use crate::TaskLogMeta;
    use reili_core::knowledge::{MockWebSearchPort, WebSearchPort};
    use reili_core::logger::{LogEntry as CoreLogEntry, LogLevel};
    use reili_core::messaging::slack::{
        MockSlackMessageSearchPort, MockSlackTaskControlMessagePort, MockSlackThreadHistoryPort,
        MockSlackThreadReplyPort, SlackMessage, SlackMessageSearchPort,
        SlackTaskControlMessagePort, SlackThreadHistoryPort, SlackTriggerType,
    };
    use reili_core::queue::{CompleteJobInput, FailJobInput, JobFailResult, MockJobQueuePort};
    use reili_core::task::{
        LlmExecutionMetadata, LlmUsageSnapshot, MockTaskProgressSessionFactoryPort,
        MockTaskProgressSessionPort, MockTaskRunnerPort, RunTaskInput, TaskJobPayload,
        TaskProgressSessionFactoryPort, TaskProgressSessionPort, TaskResources, TaskRunOutcome,
        TaskRunReport, TaskRunnerPort,
    };
    use std::sync::Mutex;

    use crate::{LogFieldValue, TaskLogger};

    use super::InFlightJobRegistry;

    const USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
        requests: 1,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
    };

    #[derive(Debug, Clone)]
    struct LogEntry {
        message: String,
        meta: TaskLogMeta,
    }

    #[derive(Default)]
    struct MockLogger {
        infos: Mutex<Vec<LogEntry>>,
        errors: Mutex<Vec<LogEntry>>,
    }

    impl MockLogger {
        fn infos(&self) -> Vec<LogEntry> {
            self.infos.lock().expect("lock infos").clone()
        }

        fn errors(&self) -> Vec<LogEntry> {
            self.errors.lock().expect("lock errors").clone()
        }
    }

    impl TaskLogger for MockLogger {
        fn log(&self, entry: CoreLogEntry) {
            let captured = LogEntry {
                message: entry.event.to_string(),
                meta: entry.fields,
            };

            match entry.level {
                LogLevel::Info => self.infos.lock().expect("lock infos").push(captured),
                LogLevel::Error => self.errors.lock().expect("lock errors").push(captured),
                LogLevel::Debug | LogLevel::Warn => {}
            }
        }
    }

    fn create_resources() -> TaskResources {
        let slack_message_search_port: Arc<dyn SlackMessageSearchPort> =
            Arc::new(MockSlackMessageSearchPort::new());
        let web_search_port: Arc<dyn WebSearchPort> = Arc::new(MockWebSearchPort::new());

        TaskResources {
            slack_message_search_port,
            web_search_port,
        }
    }

    fn create_progress_session_factory_for_execution() -> Arc<dyn TaskProgressSessionFactoryPort> {
        let mut session = MockTaskProgressSessionPort::new();
        session.expect_start().times(1).returning(|| ());
        session.expect_apply().times(0);
        session.expect_complete().times(1).returning(|_| ());

        let mut factory = MockTaskProgressSessionFactoryPort::new();
        factory
            .expect_create_for_thread()
            .times(1)
            .return_once(move |_| Box::new(session) as Box<dyn TaskProgressSessionPort>);

        Arc::new(factory) as Arc<dyn TaskProgressSessionFactoryPort>
    }

    struct TestContext {
        deps: Arc<StartTaskWorkerRunnerUseCaseDeps>,
        complete_inputs: Arc<Mutex<Vec<CompleteJobInput>>>,
        fail_inputs: Arc<Mutex<Vec<FailJobInput>>>,
        posted_replies: Arc<Mutex<Vec<SlackThreadReplyInput>>>,
        logger: Arc<MockLogger>,
    }

    fn create_success_context() -> TestContext {
        let complete_inputs = Arc::new(Mutex::new(Vec::new()));
        let fail_inputs = Arc::new(Mutex::new(Vec::new()));
        let posted_replies = Arc::new(Mutex::new(Vec::new()));
        let logger = Arc::new(MockLogger::default());
        let mut job_queue = MockJobQueuePort::<TaskJob>::new();
        let complete_calls = Arc::clone(&complete_inputs);
        job_queue
            .expect_complete()
            .times(1)
            .returning(move |input: CompleteJobInput| {
                complete_calls
                    .lock()
                    .expect("lock complete inputs")
                    .push(input);
                Ok(())
            });
        job_queue.expect_fail().times(0);
        job_queue.expect_get_depth().times(1).return_const(Ok(3));

        let mut slack_reply_port = MockSlackThreadReplyPort::new();
        let reply_calls = Arc::clone(&posted_replies);
        slack_reply_port
            .expect_post_thread_reply()
            .times(1)
            .returning(move |input: SlackThreadReplyInput| {
                reply_calls.lock().expect("lock posted replies").push(input);
                Ok(())
            });

        let mut slack_thread_history_port = MockSlackThreadHistoryPort::new();
        slack_thread_history_port
            .expect_fetch_thread_history()
            .times(0);

        let mut task_runner = MockTaskRunnerPort::new();
        task_runner
            .expect_run()
            .times(1)
            .returning(|_: RunTaskInput| {
                Ok(TaskRunOutcome::Succeeded(TaskRunReport {
                    result_text: "task_runner result".to_string(),
                    usage: USAGE_SNAPSHOT,
                    execution: LlmExecutionMetadata {
                        provider: "openai".to_string(),
                        model: "gpt-test".to_string(),
                    },
                }))
            });

        let mut slack_task_control_message_port = MockSlackTaskControlMessagePort::new();
        slack_task_control_message_port
            .expect_post_task_control_message()
            .times(0);
        slack_task_control_message_port
            .expect_update_task_control_message()
            .times(2)
            .returning(|_| Ok(()));

        let deps = Arc::new(StartTaskWorkerRunnerUseCaseDeps {
            job_queue: Arc::new(job_queue) as Arc<TaskJobQueuePort>,
            in_flight_job_registry: InFlightJobRegistry::new(),
            slack_task_control_message_port: Arc::new(slack_task_control_message_port)
                as Arc<dyn SlackTaskControlMessagePort>,
            task_execution_deps: TaskExecutionDeps {
                slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
                task_progress_session_factory_port: create_progress_session_factory_for_execution(),
                slack_thread_history_port: Arc::new(slack_thread_history_port)
                    as Arc<dyn SlackThreadHistoryPort>,
                task_resources: create_resources(),
                task_runner: Arc::new(task_runner) as Arc<dyn TaskRunnerPort>,
                logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
                slack_bot_user_id: "UBOT".to_string(),
            },
            worker_concurrency: 1,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
        });

        TestContext {
            deps,
            complete_inputs,
            fail_inputs,
            posted_replies,
            logger,
        }
    }

    fn create_failure_context(
        fail_result: JobFailResult<TaskJob>,
        queue_depth_result: Result<usize, PortError>,
        expected_reply_calls: usize,
    ) -> TestContext {
        let complete_inputs = Arc::new(Mutex::new(Vec::new()));
        let fail_inputs = Arc::new(Mutex::new(Vec::new()));
        let posted_replies = Arc::new(Mutex::new(Vec::new()));
        let logger = Arc::new(MockLogger::default());
        let mut job_queue = MockJobQueuePort::<TaskJob>::new();
        job_queue.expect_complete().times(0);
        let fail_calls = Arc::clone(&fail_inputs);
        let updates_dead_letter = fail_result.status == JobFailStatus::DeadLetter;
        job_queue
            .expect_fail()
            .times(1)
            .returning(move |input: FailJobInput| {
                fail_calls.lock().expect("lock fail inputs").push(input);
                Ok(fail_result.clone())
            });
        job_queue
            .expect_get_depth()
            .times(1)
            .return_const(queue_depth_result.clone());

        let mut slack_reply_port = MockSlackThreadReplyPort::new();
        if expected_reply_calls == 0 {
            slack_reply_port.expect_post_thread_reply().times(0);
        } else {
            let reply_calls = Arc::clone(&posted_replies);
            slack_reply_port
                .expect_post_thread_reply()
                .times(expected_reply_calls)
                .returning(move |input: SlackThreadReplyInput| {
                    reply_calls.lock().expect("lock posted replies").push(input);
                    Ok(())
                });
        }

        let mut slack_task_control_message_port = MockSlackTaskControlMessagePort::new();
        slack_task_control_message_port
            .expect_post_task_control_message()
            .times(0);
        slack_task_control_message_port
            .expect_update_task_control_message()
            .times(if updates_dead_letter { 1 } else { 0 })
            .returning(|_| Ok(()));

        let deps = Arc::new(StartTaskWorkerRunnerUseCaseDeps {
            job_queue: Arc::new(job_queue) as Arc<TaskJobQueuePort>,
            in_flight_job_registry: InFlightJobRegistry::new(),
            slack_task_control_message_port: Arc::new(slack_task_control_message_port)
                as Arc<dyn SlackTaskControlMessagePort>,
            task_execution_deps: TaskExecutionDeps {
                slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
                task_progress_session_factory_port: Arc::new(
                    MockTaskProgressSessionFactoryPort::new(),
                )
                    as Arc<dyn TaskProgressSessionFactoryPort>,
                slack_thread_history_port: Arc::new(MockSlackThreadHistoryPort::new())
                    as Arc<dyn SlackThreadHistoryPort>,
                task_resources: create_resources(),
                task_runner: Arc::new(MockTaskRunnerPort::new()) as Arc<dyn TaskRunnerPort>,
                logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
                slack_bot_user_id: "UBOT".to_string(),
            },
            worker_concurrency: 1,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
        });

        TestContext {
            deps,
            complete_inputs,
            fail_inputs,
            posted_replies,
            logger,
        }
    }

    fn create_job(input: CreateJobInput) -> TaskJob {
        TaskJob {
            job_id: input.job_id,
            received_at: "2026-03-04T00:00:00.000Z".to_string(),
            payload: TaskJobPayload {
                slack_event_id: "Ev001".to_string(),
                message: SlackMessage {
                    slack_event_id: "Ev001".to_string(),
                    team_id: Some("T001".to_string()),
                    action_token: None,
                    trigger: SlackTriggerType::Message,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    actor_is_bot: false,
                    text: "alert".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    ts: "1710000000.000001".to_string(),
                    thread_ts: input.thread_ts,
                },
                control_message_ts: "1710000000.000002".to_string(),
            },
            retry_count: input.retry_count,
        }
    }

    struct CreateJobInput {
        job_id: String,
        retry_count: u32,
        thread_ts: Option<String>,
    }

    #[tokio::test]
    async fn process_claimed_job_completes_and_logs() {
        let context = create_success_context();

        process_claimed_job(ProcessClaimedJobInput {
            deps: Arc::clone(&context.deps),
            worker_index: 0,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 0,
                thread_ts: None,
            }),
        })
        .await;

        assert_eq!(
            context
                .complete_inputs
                .lock()
                .expect("lock complete inputs")
                .len(),
            1
        );
        assert!(
            context
                .fail_inputs
                .lock()
                .expect("lock fail inputs")
                .is_empty()
        );
        assert_eq!(
            context
                .posted_replies
                .lock()
                .expect("lock posted replies")
                .len(),
            1
        );
        let info_logs = context.logger.infos();
        assert!(
            info_logs
                .iter()
                .any(|entry| entry.message == "Completed worker job")
        );
        assert_eq!(
            info_logs
                .iter()
                .find(|entry| entry.message == "Completed worker job")
                .and_then(|entry| entry.meta.get("worker_queue_depth"))
                .and_then(LogFieldValue::as_str),
            Some("3")
        );
    }

    #[tokio::test]
    async fn handle_failed_claimed_job_requeues_and_logs() {
        let context = create_failure_context(
            JobFailResult {
                status: JobFailStatus::Requeued,
                job: create_job(CreateJobInput {
                    job_id: "job-1".to_string(),
                    retry_count: 1,
                    thread_ts: None,
                }),
            },
            Ok(2),
            0,
        );

        handle_failed_claimed_job(super::HandleFailedClaimedJobInput {
            deps: Arc::clone(&context.deps),
            worker_index: 0,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 0,
                thread_ts: None,
            }),
            started_at: std::time::Instant::now(),
            error_message: "processing failed".to_string(),
            failure_disposition: super::FailureDisposition::Retryable,
        })
        .await;

        let fail_inputs = context
            .fail_inputs
            .lock()
            .expect("lock fail inputs")
            .clone();
        assert_eq!(fail_inputs.len(), 1);
        assert_eq!(fail_inputs[0].reason, "processing failed");
        assert_eq!(fail_inputs[0].max_retry, 2);
        assert_eq!(fail_inputs[0].backoff_ms, 1_000);

        let error_logs = context.logger.errors();
        assert_eq!(error_logs.len(), 1);
        assert_eq!(error_logs[0].message, "Failed worker job");
        assert_eq!(
            error_logs[0]
                .meta
                .get("status")
                .and_then(LogFieldValue::as_str),
            Some("requeued")
        );
        assert!(
            context
                .posted_replies
                .lock()
                .expect("lock posted replies")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn handle_failed_claimed_job_posts_dead_letter_message() {
        let context = create_failure_context(
            JobFailResult {
                status: JobFailStatus::DeadLetter,
                job: create_job(CreateJobInput {
                    job_id: "job-1".to_string(),
                    retry_count: 2,
                    thread_ts: Some("1710000000.000001".to_string()),
                }),
            },
            Ok(0),
            1,
        );

        handle_failed_claimed_job(super::HandleFailedClaimedJobInput {
            deps: Arc::clone(&context.deps),
            worker_index: 0,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 2,
                thread_ts: Some("1710000000.000001".to_string()),
            }),
            started_at: std::time::Instant::now(),
            error_message: "fatal failure".to_string(),
            failure_disposition: super::FailureDisposition::Retryable,
        })
        .await;

        let posted_replies = context
            .posted_replies
            .lock()
            .expect("lock posted replies")
            .clone();
        assert_eq!(posted_replies.len(), 1);
        assert_eq!(
            posted_replies[0].text,
            "Task failed after retries: fatal failure"
        );
        assert_eq!(posted_replies[0].thread_ts, "1710000000.000001");
    }

    #[tokio::test]
    async fn handle_failed_claimed_job_dead_letters_permanent_failure_immediately() {
        let context = create_failure_context(
            JobFailResult {
                status: JobFailStatus::DeadLetter,
                job: create_job(CreateJobInput {
                    job_id: "job-1".to_string(),
                    retry_count: 0,
                    thread_ts: Some("1710000000.000001".to_string()),
                }),
            },
            Ok(0),
            1,
        );

        handle_failed_claimed_job(super::HandleFailedClaimedJobInput {
            deps: Arc::clone(&context.deps),
            worker_index: 0,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 0,
                thread_ts: Some("1710000000.000001".to_string()),
            }),
            started_at: std::time::Instant::now(),
            error_message: "mcp connect failed".to_string(),
            failure_disposition: super::FailureDisposition::Permanent,
        })
        .await;

        let fail_inputs = context
            .fail_inputs
            .lock()
            .expect("lock fail inputs")
            .clone();
        assert_eq!(fail_inputs.len(), 1);
        assert_eq!(fail_inputs[0].max_retry, 0);

        let posted_replies = context
            .posted_replies
            .lock()
            .expect("lock posted replies")
            .clone();
        assert_eq!(posted_replies.len(), 1);
        assert_eq!(posted_replies[0].text, "Task failed: mcp connect failed");
    }
}
