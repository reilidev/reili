use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackThreadHistoryPort, SlackThreadReplyInput, SlackThreadReplyPort,
};
use reili_core::task::{
    LlmExecutionMetadata, LlmUsageSnapshot, RunTaskInput, TaskCancellation, TaskContext,
    TaskJobPayload, TaskProgressEventInput, TaskProgressEventPort, TaskProgressSessionFactoryPort,
    TaskRequest, TaskResources, TaskRunOutcome, TaskRunnerPort, TaskRuntime,
};
use tokio::sync::{Mutex, mpsc};

use crate::progress::{
    CreateTaskProgressStreamSessionFactoryInput, CreateTaskProgressStreamSessionInput,
    TaskProgressEventHandler, TaskProgressEventHandlerInput, TaskProgressStreamSession,
    TaskProgressStreamSessionFactory, create_task_progress_stream_session_factory,
};
use crate::{LogFieldValue, TaskLogMeta, TaskLogger, string_log_meta};

use super::errors::{ExecuteTaskJobError, resolve_task_failure_error};
use super::memory_context::{
    SlackMemoryContextLoader, SlackMemoryContextLoaderDeps, SlackMemoryContextLoaderInput,
};
use super::thread_context::{
    SlackThreadContextLoader, SlackThreadContextLoaderDeps, SlackThreadContextLoaderInput,
};

const FALLBACK_REPORT_TEXT: &str = "Task completed but failed to generate a report.";

#[derive(Clone)]
pub struct TaskExecutionDeps {
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub task_progress_session_factory_port: Arc<dyn TaskProgressSessionFactoryPort>,
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub task_resources: TaskResources,
    pub task_runner: Arc<dyn TaskRunnerPort>,
    pub logger: Arc<dyn TaskLogger>,
    pub slack_bot_user_id: String,
}

pub struct ExecuteTaskJobInput {
    pub job_id: String,
    pub retry_count: u32,
    pub payload: TaskJobPayload,
    pub task_cancellation: TaskCancellation,
    pub deps: TaskExecutionDeps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskExecutionOutcome {
    Succeeded,
    Cancelled,
}

pub async fn execute_task_job(
    input: ExecuteTaskJobInput,
) -> Result<TaskExecutionOutcome, ExecuteTaskJobError> {
    let thread_ts = input.payload.message.thread_ts_or_ts().to_string();
    let started_at = Instant::now();
    let started_at_utc = Utc::now();
    let started_at_iso = started_at_utc.to_rfc3339_opts(SecondsFormat::Millis, true);
    let started_at_unix_seconds = started_at_utc.timestamp();

    let base_log_meta = string_log_meta([
        ("slackEventId", input.payload.slack_event_id.clone()),
        ("jobId", input.job_id.clone()),
        ("channel", input.payload.message.channel.clone()),
        ("threadTs", thread_ts.clone()),
        ("attempt", (input.retry_count + 1).to_string()),
    ]);

    let progress_session_factory =
        create_task_progress_stream_session_factory(CreateTaskProgressStreamSessionFactoryInput {
            progress_session_factory_port: Arc::clone(
                &input.deps.task_progress_session_factory_port,
            ),
            logger: Arc::clone(&input.deps.logger),
        });
    let progress_session: Arc<Mutex<Box<dyn TaskProgressStreamSession>>> = Arc::new(Mutex::new(
        progress_session_factory.create_for_thread(CreateTaskProgressStreamSessionInput {
            channel: input.payload.message.channel.clone(),
            thread_ts: thread_ts.clone(),
            recipient_user_id: input.payload.message.user.clone(),
            recipient_team_id: input.payload.message.team_id.clone(),
        }),
    ));

    let progress_event_handler = TaskProgressEventHandler::new(TaskProgressEventHandlerInput {
        progress_session: Arc::clone(&progress_session),
    });
    let (progress_event_sender, progress_event_receiver) =
        mpsc::unbounded_channel::<TaskProgressEventInput>();
    let on_progress_event: Arc<dyn TaskProgressEventPort> =
        Arc::new(ChannelProgressEventPort::new(progress_event_sender));
    let progress_event_task = tokio::spawn(run_progress_event_loop(
        progress_event_receiver,
        progress_event_handler,
    ));

    let thread_context_loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
        slack_thread_history_port: Arc::clone(&input.deps.slack_thread_history_port),
        logger: Arc::clone(&input.deps.logger),
    });

    let execution_result = run_task(RunTaskStageInput {
        retry_count: input.retry_count,
        payload: input.payload.clone(),
        task_cancellation: input.task_cancellation.clone(),
        deps: input.deps.clone(),
        thread_ts: thread_ts.clone(),
        started_at_iso: started_at_iso.clone(),
        started_at_unix_seconds,
        base_log_meta: base_log_meta.clone(),
        progress_session: Arc::clone(&progress_session),
        on_progress_event: Arc::clone(&on_progress_event),
        thread_context_loader,
    })
    .await;

    drop(on_progress_event);
    let _ = progress_event_task.await;

    match execution_result {
        Ok(success) => {
            {
                let mut session = progress_session.lock().await;
                match success {
                    TaskExecutionSuccess::Succeeded(success) => {
                        session.stop_as_succeeded().await;
                        drop(session);

                        post_slack_reply_stage(
                            Arc::clone(&input.deps.slack_reply_port),
                            input.payload.message.channel.clone(),
                            thread_ts,
                            success.report_text,
                            success.llm_usage.clone(),
                        )
                        .await?;

                        let duration_ms = started_at.elapsed().as_millis();
                        let mut meta = merge_log_meta(
                            &base_log_meta,
                            &build_llm_token_log_meta(&success.llm_usage),
                        );
                        meta = merge_log_meta(
                            &meta,
                            &build_llm_execution_log_meta(&success.llm_execution),
                        );
                        meta.insert(
                            "worker_job_duration_ms".to_string(),
                            LogFieldValue::from(duration_ms),
                        );
                        meta.insert("latencyMs".to_string(), LogFieldValue::from(duration_ms));
                        input.deps.logger.info("Processed task job", meta);
                        return Ok(TaskExecutionOutcome::Succeeded);
                    }
                    TaskExecutionSuccess::Cancelled => {
                        session.stop_as_cancelled().await;
                    }
                }
            }
            let duration_ms = started_at.elapsed().as_millis();
            let mut meta = base_log_meta.clone();
            meta.insert(
                "worker_job_duration_ms".to_string(),
                LogFieldValue::from(duration_ms),
            );
            meta.insert("latencyMs".to_string(), LogFieldValue::from(duration_ms));
            meta.insert("status".to_string(), LogFieldValue::from("cancelled"));
            input.deps.logger.info("Cancelled task job", meta);
            Ok(TaskExecutionOutcome::Cancelled)
        }
        Err(error) => {
            {
                let mut session = progress_session.lock().await;
                session.stop_as_failed().await;
            }

            let failure_error = resolve_task_failure_error(&error);

            let duration_ms = started_at.elapsed().as_millis();
            let mut meta = merge_log_meta(
                &base_log_meta,
                &build_llm_token_log_meta(&failure_error.usage),
            );
            meta.insert(
                "worker_job_duration_ms".to_string(),
                LogFieldValue::from(duration_ms),
            );
            meta.insert("latencyMs".to_string(), LogFieldValue::from(duration_ms));
            meta.insert(
                "error".to_string(),
                LogFieldValue::from(failure_error.error_message),
            );
            input.deps.logger.error("Failed task job", meta);
            Err(error)
        }
    }
}

enum TaskExecutionSuccess {
    Succeeded(TaskExecutionRunSuccess),
    Cancelled,
}

struct TaskExecutionRunSuccess {
    report_text: String,
    llm_usage: LlmUsageSnapshot,
    llm_execution: LlmExecutionMetadata,
}

struct RunTaskStageInput {
    retry_count: u32,
    payload: TaskJobPayload,
    task_cancellation: TaskCancellation,
    deps: TaskExecutionDeps,
    thread_ts: String,
    started_at_iso: String,
    started_at_unix_seconds: i64,
    base_log_meta: TaskLogMeta,
    progress_session: Arc<Mutex<Box<dyn TaskProgressStreamSession>>>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    thread_context_loader: SlackThreadContextLoader,
}

async fn run_task(input: RunTaskStageInput) -> Result<TaskExecutionSuccess, ExecuteTaskJobError> {
    let thread_messages = input
        .thread_context_loader
        .load_for_message(SlackThreadContextLoaderInput {
            message: input.payload.message.clone(),
            base_log_meta: input.base_log_meta.clone(),
        })
        .await;

    let memory_context_loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
        slack_message_search_port: Arc::clone(&input.deps.task_resources.slack_message_search_port),
        logger: Arc::clone(&input.deps.logger),
        bot_user_id: input.deps.slack_bot_user_id.clone(),
    });
    let memory_items = memory_context_loader
        .load_for_message(SlackMemoryContextLoaderInput {
            message: input.payload.message.clone(),
            started_at_unix_seconds: input.started_at_unix_seconds,
            base_log_meta: input.base_log_meta.clone(),
        })
        .await;

    let request = TaskRequest {
        trigger_message: input.payload.message.clone(),
        thread_messages,
        memory_items,
    };

    let runtime = TaskRuntime {
        started_at_iso: input.started_at_iso,
        channel: input.payload.message.channel.clone(),
        thread_ts: input.thread_ts,
        retry_count: input.retry_count,
    };
    let context = TaskContext {
        resources: input.deps.task_resources.clone(),
        runtime,
        cancellation: input.task_cancellation.clone(),
    };

    {
        let mut session = input.progress_session.lock().await;
        session.start().await;
    }

    let task_runner_future = input.deps.task_runner.run(RunTaskInput {
        request,
        context,
        on_progress_event: Arc::clone(&input.on_progress_event),
        logger: Arc::clone(&input.deps.logger),
    });
    tokio::pin!(task_runner_future);

    let task_runner_result = tokio::select! {
        result = &mut task_runner_future => Some(result),
        _ = input.task_cancellation.wait_for_cancellation() => None,
    };

    let task_outcome = match task_runner_result {
        Some(Ok(outcome)) => {
            if input.task_cancellation.is_cancelled() {
                TaskRunOutcome::Cancelled
            } else {
                outcome
            }
        }
        Some(Err(error)) => {
            if input.task_cancellation.is_cancelled() {
                return Ok(TaskExecutionSuccess::Cancelled);
            }
            return Err(ExecuteTaskJobError::from(error));
        }
        None => return Ok(TaskExecutionSuccess::Cancelled),
    };

    let task_report = match task_outcome {
        TaskRunOutcome::Succeeded(report) => report,
        TaskRunOutcome::Cancelled => return Ok(TaskExecutionSuccess::Cancelled),
    };

    let report_text = if task_report.result_text.is_empty() {
        FALLBACK_REPORT_TEXT.to_string()
    } else {
        task_report.result_text
    };

    Ok(TaskExecutionSuccess::Succeeded(TaskExecutionRunSuccess {
        report_text,
        llm_usage: task_report.usage,
        llm_execution: task_report.execution,
    }))
}

fn build_llm_execution_log_meta(execution: &LlmExecutionMetadata) -> TaskLogMeta {
    string_log_meta([
        ("llm_provider", execution.provider.clone()),
        ("llm_model", execution.model.clone()),
    ])
}

async fn post_slack_reply_stage(
    slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    channel: String,
    thread_ts: String,
    report_text: String,
    llm_usage: LlmUsageSnapshot,
) -> Result<(), ExecuteTaskJobError> {
    slack_reply_port
        .post_thread_reply(SlackThreadReplyInput {
            channel,
            thread_ts,
            text: report_text,
        })
        .await
        .map_err(|error| {
            ExecuteTaskJobError::TaskExecutionFailed(
                reili_core::error::TaskExecutionFailedError::new(error.message, llm_usage),
            )
        })
}

fn build_llm_token_log_meta(usage: &LlmUsageSnapshot) -> TaskLogMeta {
    string_log_meta([
        (
            "llm_tokens_input_total",
            LogFieldValue::from(usage.input_tokens),
        ),
        (
            "llm_tokens_output_total",
            LogFieldValue::from(usage.output_tokens),
        ),
        ("llm_tokens_total", LogFieldValue::from(usage.total_tokens)),
        ("llm_requests_total", LogFieldValue::from(usage.requests)),
    ])
}

fn merge_log_meta(base: &TaskLogMeta, append: &TaskLogMeta) -> TaskLogMeta {
    let mut merged = base.clone();
    merged.extend(append.clone());
    merged
}

struct ChannelProgressEventPort {
    sender: mpsc::UnboundedSender<TaskProgressEventInput>,
}

impl ChannelProgressEventPort {
    fn new(sender: mpsc::UnboundedSender<TaskProgressEventInput>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl TaskProgressEventPort for ChannelProgressEventPort {
    async fn publish(&self, input: TaskProgressEventInput) -> Result<(), PortError> {
        self.sender.send(input).map_err(|send_error| {
            PortError::new(format!(
                "Failed to enqueue progress event for handling: {send_error}"
            ))
        })
    }
}

async fn run_progress_event_loop(
    mut receiver: mpsc::UnboundedReceiver<TaskProgressEventInput>,
    handler: TaskProgressEventHandler,
) {
    while let Some(event) = receiver.recv().await {
        handler.handle(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::knowledge::{MockWebSearchPort, WebSearchPort};
    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        FetchSlackThreadHistoryInput, MockSlackMessageSearchPort, MockSlackThreadHistoryPort,
        MockSlackThreadReplyPort, SlackMessage, SlackMessageSearchContextMessages,
        SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchResult,
        SlackMessageSearchResultItem, SlackThreadHistoryPort, SlackThreadMessage,
        SlackThreadReplyInput, SlackThreadReplyPort, SlackTriggerType,
    };
    use reili_core::secret::SecretString;
    use reili_core::task::{
        LlmExecutionMetadata, LlmUsageSnapshot, MockTaskProgressSessionFactoryPort,
        MockTaskProgressSessionPort, MockTaskRunnerPort, RunTaskInput, TaskCancellation,
        TaskJobPayload, TaskProgressSessionFactoryPort, TaskProgressSessionPort, TaskRequest,
        TaskResources, TaskRunOutcome, TaskRunReport, TaskRunnerPort, TaskRuntime,
    };

    use super::{ExecuteTaskJobInput, TaskExecutionDeps, execute_task_job};
    use crate::{LogFieldValue, TaskLogger};

    const USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
        requests: 1,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedTaskRunInput {
        request: TaskRequest,
        runtime: TaskRuntime,
    }

    struct NoopLogger;

    impl TaskLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
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

    fn create_progress_session_factory() -> Arc<dyn TaskProgressSessionFactoryPort> {
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

    fn create_payload(ts: &str, thread_ts: Option<&str>, text: Option<&str>) -> TaskJobPayload {
        TaskJobPayload {
            slack_event_id: "Ev001".to_string(),
            message: SlackMessage {
                slack_event_id: "Ev001".to_string(),
                team_id: Some("T001".to_string()),
                action_token: None,
                trigger: SlackTriggerType::AppMention,
                channel: "C001".to_string(),
                user: "U001".to_string(),
                actor_is_bot: false,
                text: text.unwrap_or("monitor alert").to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                ts: ts.to_string(),
                thread_ts: thread_ts.map(ToString::to_string),
            },
            control_message_ts: "1710000000.000002".to_string(),
        }
    }

    struct ExecutionFixtures {
        deps: TaskExecutionDeps,
        slack_reply_calls: Arc<Mutex<Vec<SlackThreadReplyInput>>>,
        slack_thread_history_calls: Arc<Mutex<Vec<FetchSlackThreadHistoryInput>>>,
        slack_message_search_calls: Arc<Mutex<Vec<SlackMessageSearchInput>>>,
        task_runs: Arc<Mutex<Vec<CapturedTaskRunInput>>>,
    }

    fn create_execution_fixtures(
        thread_history_response: Option<Result<Vec<SlackThreadMessage>, PortError>>,
    ) -> ExecutionFixtures {
        create_execution_fixtures_with_memory_search(thread_history_response, None)
    }

    fn create_execution_fixtures_with_memory_search(
        thread_history_response: Option<Result<Vec<SlackThreadMessage>, PortError>>,
        memory_search_response: Option<Result<SlackMessageSearchResult, PortError>>,
    ) -> ExecutionFixtures {
        let slack_reply_calls = Arc::new(Mutex::new(Vec::new()));
        let slack_thread_history_calls = Arc::new(Mutex::new(Vec::new()));
        let slack_message_search_calls = Arc::new(Mutex::new(Vec::new()));
        let task_runs = Arc::new(Mutex::new(Vec::new()));

        let mut slack_reply_port = MockSlackThreadReplyPort::new();
        let reply_calls = Arc::clone(&slack_reply_calls);
        slack_reply_port
            .expect_post_thread_reply()
            .times(1)
            .returning(move |input: SlackThreadReplyInput| {
                reply_calls.lock().expect("lock reply calls").push(input);
                Ok(())
            });

        let mut slack_thread_history_port = MockSlackThreadHistoryPort::new();
        match thread_history_response {
            Some(response) => {
                let history_calls = Arc::clone(&slack_thread_history_calls);
                slack_thread_history_port
                    .expect_fetch_thread_history()
                    .times(1)
                    .returning(move |input: FetchSlackThreadHistoryInput| {
                        history_calls
                            .lock()
                            .expect("lock history calls")
                            .push(input);
                        response.clone()
                    });
            }
            None => {
                slack_thread_history_port
                    .expect_fetch_thread_history()
                    .times(0);
            }
        }

        let mut task_runner = MockTaskRunnerPort::new();
        let captured_runs = Arc::clone(&task_runs);
        task_runner
            .expect_run()
            .times(1)
            .returning(move |input: RunTaskInput| {
                captured_runs
                    .lock()
                    .expect("lock captured runs")
                    .push(CapturedTaskRunInput {
                        request: input.request,
                        runtime: input.context.runtime,
                    });

                Ok(TaskRunOutcome::Succeeded(TaskRunReport {
                    result_text: "task_runner result".to_string(),
                    usage: USAGE_SNAPSHOT,
                    execution: LlmExecutionMetadata {
                        provider: "openai".to_string(),
                        model: "gpt-test".to_string(),
                    },
                }))
            });

        let mut slack_message_search_port = MockSlackMessageSearchPort::new();
        match memory_search_response {
            Some(response) => {
                let search_calls = Arc::clone(&slack_message_search_calls);
                slack_message_search_port
                    .expect_search_messages()
                    .times(1)
                    .returning(move |input: SlackMessageSearchInput| {
                        search_calls.lock().expect("lock search calls").push(input);
                        response.clone()
                    });
            }
            None => {
                slack_message_search_port.expect_search_messages().times(0);
            }
        }

        let deps = TaskExecutionDeps {
            slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            task_progress_session_factory_port: create_progress_session_factory(),
            slack_thread_history_port: Arc::new(slack_thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            task_resources: TaskResources {
                slack_message_search_port: Arc::new(slack_message_search_port)
                    as Arc<dyn SlackMessageSearchPort>,
                web_search_port: create_resources().web_search_port,
            },
            task_runner: Arc::new(task_runner) as Arc<dyn TaskRunnerPort>,
            logger: Arc::new(NoopLogger) as Arc<dyn TaskLogger>,
            slack_bot_user_id: "UBOT".to_string(),
        };

        ExecutionFixtures {
            deps,
            slack_reply_calls,
            slack_thread_history_calls,
            slack_message_search_calls,
            task_runs,
        }
    }

    #[tokio::test]
    async fn fetches_thread_history_only_for_thread_replies() {
        let fixtures = create_execution_fixtures(Some(Ok(vec![SlackThreadMessage {
            ts: "1710000000.000001".to_string(),
            user: Some("U999".to_string()),
            text: "thread context".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            metadata: None,
        }])));
        let ExecutionFixtures {
            deps,
            slack_reply_calls,
            slack_thread_history_calls,
            slack_message_search_calls,
            task_runs,
        } = fixtures;

        let result = execute_task_job(ExecuteTaskJobInput {
            job_id: "job-1".to_string(),
            retry_count: 0,
            payload: create_payload(
                "1710000000.000002",
                Some("1710000000.000001"),
                Some("<@U999> monitor alert"),
            ),
            task_cancellation: TaskCancellation::new(),
            deps,
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(
            slack_thread_history_calls
                .lock()
                .expect("lock history calls")
                .clone(),
            vec![FetchSlackThreadHistoryInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
            }]
        );

        let captured = task_runs.lock().expect("lock captured runs").clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].request.trigger_message.text,
            "<@U999> monitor alert"
        );
        assert_eq!(captured[0].request.thread_messages.len(), 1);
        assert_eq!(
            captured[0].request.thread_messages[0].text,
            "thread context"
        );
        assert!(captured[0].request.memory_items.is_empty());
        assert!(
            slack_message_search_calls
                .lock()
                .expect("lock search calls")
                .is_empty()
        );
        assert_eq!(
            slack_reply_calls.lock().expect("lock reply calls").clone(),
            vec![SlackThreadReplyInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                text: "task_runner result".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn does_not_fetch_thread_history_for_non_thread_messages() {
        let fixtures = create_execution_fixtures(None);
        let ExecutionFixtures {
            deps,
            slack_thread_history_calls,
            task_runs,
            ..
        } = fixtures;

        let result = execute_task_job(ExecuteTaskJobInput {
            job_id: "job-2".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000100", None, None),
            task_cancellation: TaskCancellation::new(),
            deps,
        })
        .await;

        assert!(result.is_ok());
        assert!(
            slack_thread_history_calls
                .lock()
                .expect("lock history calls")
                .is_empty()
        );

        let captured = task_runs.lock().expect("lock captured runs").clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].request.thread_messages.is_empty());
        assert!(captured[0].request.memory_items.is_empty());
    }

    #[tokio::test]
    async fn falls_back_when_thread_history_fetch_fails() {
        let fixtures = create_execution_fixtures(Some(Err(PortError::new("slack api failed"))));
        let ExecutionFixtures {
            deps, task_runs, ..
        } = fixtures;

        let result = execute_task_job(ExecuteTaskJobInput {
            job_id: "job-3".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000200", Some("1710000000.000150"), None),
            task_cancellation: TaskCancellation::new(),
            deps,
        })
        .await;

        assert!(result.is_ok());

        let captured = task_runs.lock().expect("lock captured runs").clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].request.thread_messages.is_empty());
    }

    #[tokio::test]
    async fn includes_slack_memory_context_when_action_token_is_available() {
        let fixtures = create_execution_fixtures_with_memory_search(
            None,
            Some(Ok(SlackMessageSearchResult {
                messages: vec![SlackMessageSearchResultItem {
                    author_name: Some("Reili".to_string()),
                    author_user_id: Some("UBOT".to_string()),
                    team_id: Some("T001".to_string()),
                    channel_id: Some("C001".to_string()),
                    channel_name: Some("alerts".to_string()),
                    message_ts: "1760000000.000001".to_string(),
                    thread_ts: Some("1760000000.000000".to_string()),
                    content: "*Reusable notes*\nreili_memory_v1\n- service: checkout-api"
                        .to_string(),
                    is_author_bot: true,
                    permalink: Some("https://slack/memory".to_string()),
                    context_messages: SlackMessageSearchContextMessages {
                        before: Vec::new(),
                        after: Vec::new(),
                    },
                }],
                next_cursor: None,
            })),
        );
        let ExecutionFixtures {
            deps,
            slack_message_search_calls,
            task_runs,
            ..
        } = fixtures;
        let mut payload = create_payload("1760000100.000001", None, None);
        payload.message.action_token = Some(SecretString::from("action-token"));

        let result = execute_task_job(ExecuteTaskJobInput {
            job_id: "job-4".to_string(),
            retry_count: 0,
            payload,
            task_cancellation: TaskCancellation::new(),
            deps,
        })
        .await;

        assert!(result.is_ok());
        let captured = task_runs.lock().expect("lock captured runs").clone();
        assert_eq!(captured[0].request.memory_items.len(), 1);
        assert_eq!(
            captured[0].request.memory_items[0].content,
            "- service: checkout-api"
        );

        let calls = slack_message_search_calls
            .lock()
            .expect("lock search calls")
            .clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].query, "reili_memory_v1");
        assert_eq!(calls[0].context_channel_id, Some("C001".to_string()));
        assert_eq!(calls[0].limit, 10);
    }

    #[test]
    fn token_log_meta_omits_task_runner_specific_total_tokens() {
        let meta = super::build_llm_token_log_meta(&LlmUsageSnapshot {
            requests: 2,
            input_tokens: 40,
            output_tokens: 60,
            total_tokens: 100,
        });

        assert_eq!(
            meta.get("llm_tokens_total"),
            Some(&LogFieldValue::from(100_u64))
        );
        assert!(!meta.contains_key("llm_tokens_total_task_runner"));
    }
}
