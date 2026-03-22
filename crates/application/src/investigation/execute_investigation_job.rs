use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::investigation::{
    InvestigationContext, InvestigationJobPayload, InvestigationLeadRunnerPort,
    InvestigationProgressEventInput, InvestigationProgressEventPort,
    InvestigationProgressSessionFactoryPort, InvestigationResources, InvestigationRuntime,
    LlmExecutionMetadata, LlmUsageSnapshot, RunInvestigationLeadInput,
};
use reili_core::messaging::slack::{
    SlackThreadHistoryPort, SlackThreadReplyInput, SlackThreadReplyPort,
};
use tokio::sync::{Mutex, mpsc};

use super::execution_errors::{ExecuteInvestigationJobError, resolve_investigation_failure_error};
use super::logger::{InvestigationLogMeta, InvestigationLogger, LogFieldValue, string_log_meta};
use super::services::{
    CreateInvestigationProgressStreamSessionFactoryInput,
    CreateInvestigationProgressStreamSessionInput, InvestigationLeadProgressEventHandler,
    InvestigationLeadProgressEventHandlerInput, InvestigationProgressStreamSession,
    InvestigationProgressStreamSessionFactory,
    create_investigation_progress_stream_session_factory,
};
use super::slack_thread_context_loader::{
    SlackThreadContextLoader, SlackThreadContextLoaderDeps, SlackThreadContextLoaderInput,
};
use crate::alert_intake::{ExtractAlertContextInput, extract_alert_context};

const FALLBACK_REPORT_TEXT: &str = "Investigation completed but failed to generate a report.";

#[derive(Clone)]
pub struct InvestigationExecutionDeps {
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub investigation_progress_session_factory_port:
        Arc<dyn InvestigationProgressSessionFactoryPort>,
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub investigation_resources: InvestigationResources,
    pub investigation_lead_runner: Arc<dyn InvestigationLeadRunnerPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

pub struct ExecuteInvestigationJobInput {
    pub job_id: String,
    pub retry_count: u32,
    pub payload: InvestigationJobPayload,
    pub deps: InvestigationExecutionDeps,
}

pub async fn execute_investigation_job(
    input: ExecuteInvestigationJobInput,
) -> Result<(), ExecuteInvestigationJobError> {
    let thread_ts = input.payload.message.thread_ts_or_ts().to_string();
    let started_at = Instant::now();
    let started_at_iso = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

    let base_log_meta = string_log_meta([
        ("slackEventId", input.payload.slack_event_id.clone()),
        ("jobId", input.job_id.clone()),
        ("channel", input.payload.message.channel.clone()),
        ("threadTs", thread_ts.clone()),
        ("attempt", (input.retry_count + 1).to_string()),
    ]);

    let progress_session_factory = create_investigation_progress_stream_session_factory(
        CreateInvestigationProgressStreamSessionFactoryInput {
            progress_session_factory_port: Arc::clone(
                &input.deps.investigation_progress_session_factory_port,
            ),
            logger: Arc::clone(&input.deps.logger),
        },
    );
    let progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>> =
        Arc::new(Mutex::new(progress_session_factory.create_for_thread(
            CreateInvestigationProgressStreamSessionInput {
                channel: input.payload.message.channel.clone(),
                thread_ts: thread_ts.clone(),
                recipient_user_id: input.payload.message.user.clone(),
                recipient_team_id: input.payload.message.team_id.clone(),
            },
        )));

    let progress_event_handler =
        InvestigationLeadProgressEventHandler::new(InvestigationLeadProgressEventHandlerInput {
            progress_session: Arc::clone(&progress_session),
        });
    let (progress_event_sender, progress_event_receiver) =
        mpsc::unbounded_channel::<InvestigationProgressEventInput>();
    let on_progress_event: Arc<dyn InvestigationProgressEventPort> =
        Arc::new(ChannelProgressEventPort::new(progress_event_sender));
    let progress_event_task = tokio::spawn(run_progress_event_loop(
        progress_event_receiver,
        progress_event_handler,
    ));

    let thread_context_loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
        slack_thread_history_port: Arc::clone(&input.deps.slack_thread_history_port),
        logger: Arc::clone(&input.deps.logger),
    });

    let execution_result = run_investigation(
        &input,
        &thread_ts,
        &started_at_iso,
        &base_log_meta,
        Arc::clone(&progress_session),
        Arc::clone(&on_progress_event),
        thread_context_loader,
    )
    .await;

    drop(on_progress_event);
    let _ = progress_event_task.await;

    match execution_result {
        Ok(success) => {
            {
                let mut session = progress_session.lock().await;
                session.stop_as_succeeded().await;
            }

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
            meta = merge_log_meta(&meta, &build_llm_execution_log_meta(&success.llm_execution));
            meta.insert(
                "worker_job_duration_ms".to_string(),
                LogFieldValue::from(duration_ms),
            );
            meta.insert("latencyMs".to_string(), LogFieldValue::from(duration_ms));
            input.deps.logger.info("Processed investigation job", meta);
            Ok(())
        }
        Err(error) => {
            {
                let mut session = progress_session.lock().await;
                session.stop_as_failed().await;
            }

            let failure_error = resolve_investigation_failure_error(&error);

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
            input.deps.logger.error("Failed investigation job", meta);
            Err(error)
        }
    }
}

struct InvestigationExecutionSuccess {
    report_text: String,
    llm_usage: LlmUsageSnapshot,
    llm_execution: LlmExecutionMetadata,
}

async fn run_investigation(
    input: &ExecuteInvestigationJobInput,
    thread_ts: &str,
    started_at_iso: &str,
    base_log_meta: &InvestigationLogMeta,
    progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>>,
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    thread_context_loader: SlackThreadContextLoader,
) -> Result<InvestigationExecutionSuccess, ExecuteInvestigationJobError> {
    let thread_messages = thread_context_loader
        .load_for_message(SlackThreadContextLoaderInput {
            message: input.payload.message.clone(),
            base_log_meta: base_log_meta.clone(),
        })
        .await;

    let alert_context = extract_alert_context(ExtractAlertContextInput {
        trigger_message_text: input.payload.message.text.clone(),
        thread_messages,
        bot_user_id: extract_mentioned_user_id(&input.payload.message.text),
    });

    let runtime = InvestigationRuntime {
        started_at_iso: started_at_iso.to_string(),
        channel: input.payload.message.channel.clone(),
        thread_ts: thread_ts.to_string(),
        retry_count: input.retry_count,
    };
    let context = InvestigationContext {
        resources: input.deps.investigation_resources.clone(),
        runtime,
    };

    {
        let mut session = progress_session.lock().await;
        session.start().await;
    }

    let investigation_lead_report = input
        .deps
        .investigation_lead_runner
        .run(RunInvestigationLeadInput {
            alert_context: alert_context.clone(),
            context,
            on_progress_event: Arc::clone(&on_progress_event),
        })
        .await
        .map_err(ExecuteInvestigationJobError::from)?;

    let report_text = if investigation_lead_report.result_text.is_empty() {
        FALLBACK_REPORT_TEXT.to_string()
    } else {
        investigation_lead_report.result_text
    };

    Ok(InvestigationExecutionSuccess {
        report_text,
        llm_usage: investigation_lead_report.usage,
        llm_execution: investigation_lead_report.execution,
    })
}

fn build_llm_execution_log_meta(execution: &LlmExecutionMetadata) -> InvestigationLogMeta {
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
) -> Result<(), ExecuteInvestigationJobError> {
    slack_reply_port
        .post_thread_reply(SlackThreadReplyInput {
            channel,
            thread_ts,
            text: report_text,
        })
        .await
        .map_err(|error| {
            ExecuteInvestigationJobError::InvestigationExecutionFailed(
                reili_core::error::InvestigationExecutionFailedError::new(error.message, llm_usage),
            )
        })
}

fn build_llm_token_log_meta(usage: &LlmUsageSnapshot) -> InvestigationLogMeta {
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

fn merge_log_meta(
    base: &InvestigationLogMeta,
    append: &InvestigationLogMeta,
) -> InvestigationLogMeta {
    let mut merged = base.clone();
    merged.extend(append.clone());
    merged
}

fn extract_mentioned_user_id(text: &str) -> Option<String> {
    let start_index = text.find("<@")?;
    let remaining = &text[start_index + 2..];
    let end_index = remaining.find('>')?;
    let user_id = &remaining[..end_index];
    if user_id.is_empty() {
        return None;
    }

    if !user_id
        .chars()
        .all(|value| value.is_ascii_uppercase() || value.is_ascii_digit())
    {
        return None;
    }

    Some(user_id.to_string())
}

struct ChannelProgressEventPort {
    sender: mpsc::UnboundedSender<InvestigationProgressEventInput>,
}

impl ChannelProgressEventPort {
    fn new(sender: mpsc::UnboundedSender<InvestigationProgressEventInput>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl InvestigationProgressEventPort for ChannelProgressEventPort {
    async fn publish(&self, input: InvestigationProgressEventInput) -> Result<(), PortError> {
        self.sender.send(input).map_err(|send_error| {
            PortError::new(format!(
                "Failed to enqueue progress event for handling: {send_error}"
            ))
        })
    }
}

async fn run_progress_event_loop(
    mut receiver: mpsc::UnboundedReceiver<InvestigationProgressEventInput>,
    handler: InvestigationLeadProgressEventHandler,
) {
    while let Some(event) = receiver.recv().await {
        handler.handle(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::investigation::{
        AlertContext, InvestigationJobPayload, InvestigationLeadRunReport,
        InvestigationLeadRunnerPort, InvestigationProgressSessionFactoryPort,
        InvestigationProgressSessionPort, InvestigationResources, InvestigationRuntime,
        LlmExecutionMetadata, LlmUsageSnapshot, MockInvestigationLeadRunnerPort,
        MockInvestigationProgressSessionFactoryPort, MockInvestigationProgressSessionPort,
        RunInvestigationLeadInput,
    };
    use reili_core::knowledge::{MockWebSearchPort, WebSearchPort};
    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        FetchSlackThreadHistoryInput, MockSlackThreadHistoryPort, MockSlackThreadReplyPort,
        SlackMessage, SlackThreadHistoryPort, SlackThreadMessage, SlackThreadReplyInput,
        SlackThreadReplyPort, SlackTriggerType,
    };
    use reili_core::monitoring::datadog::{
        DatadogEventSearchPort, DatadogLogAggregatePort, DatadogLogSearchPort,
        DatadogMetricCatalogPort, DatadogMetricQueryPort, MockDatadogEventSearchPort,
        MockDatadogLogAggregatePort, MockDatadogLogSearchPort, MockDatadogMetricCatalogPort,
        MockDatadogMetricQueryPort,
    };
    use reili_core::source_code::github::{
        GithubCodeSearchPort, GithubPullRequestPort, GithubRepositoryContentPort,
        MockGithubCodeSearchPort, MockGithubPullRequestPort, MockGithubRepositoryContentPort,
    };

    use super::{
        ExecuteInvestigationJobInput, InvestigationExecutionDeps, execute_investigation_job,
    };
    use crate::investigation::logger::{InvestigationLogger, LogFieldValue};

    const USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
        requests: 1,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedInvestigationLeadRunInput {
        alert_context: AlertContext,
        runtime: InvestigationRuntime,
    }

    struct NoopLogger;

    impl InvestigationLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    fn create_resources() -> InvestigationResources {
        let log_aggregate_port: Arc<dyn DatadogLogAggregatePort> =
            Arc::new(MockDatadogLogAggregatePort::new());
        let log_search_port: Arc<dyn DatadogLogSearchPort> =
            Arc::new(MockDatadogLogSearchPort::new());
        let metric_catalog_port: Arc<dyn DatadogMetricCatalogPort> =
            Arc::new(MockDatadogMetricCatalogPort::new());
        let metric_query_port: Arc<dyn DatadogMetricQueryPort> =
            Arc::new(MockDatadogMetricQueryPort::new());
        let event_search_port: Arc<dyn DatadogEventSearchPort> =
            Arc::new(MockDatadogEventSearchPort::new());
        let github_code_search_port: Arc<dyn GithubCodeSearchPort> =
            Arc::new(MockGithubCodeSearchPort::new());
        let github_repository_content_port: Arc<dyn GithubRepositoryContentPort> =
            Arc::new(MockGithubRepositoryContentPort::new());
        let github_pull_request_port: Arc<dyn GithubPullRequestPort> =
            Arc::new(MockGithubPullRequestPort::new());
        let web_search_port: Arc<dyn WebSearchPort> = Arc::new(MockWebSearchPort::new());

        InvestigationResources {
            log_aggregate_port,
            log_search_port,
            metric_catalog_port,
            metric_query_port,
            event_search_port,
            github_code_search_port,
            github_repository_content_port,
            github_pull_request_port,
            web_search_port,

        }
    }

    fn create_progress_session_factory() -> Arc<dyn InvestigationProgressSessionFactoryPort> {
        let mut session = MockInvestigationProgressSessionPort::new();
        session.expect_start().times(1).returning(|| ());
        session.expect_apply().times(0);
        session.expect_complete().times(1).returning(|_| ());

        let mut factory = MockInvestigationProgressSessionFactoryPort::new();
        factory
            .expect_create_for_thread()
            .times(1)
            .return_once(move |_| Box::new(session) as Box<dyn InvestigationProgressSessionPort>);

        Arc::new(factory) as Arc<dyn InvestigationProgressSessionFactoryPort>
    }

    fn create_payload(
        ts: &str,
        thread_ts: Option<&str>,
        text: Option<&str>,
    ) -> InvestigationJobPayload {
        InvestigationJobPayload {
            slack_event_id: "Ev001".to_string(),
            message: SlackMessage {
                slack_event_id: "Ev001".to_string(),
                team_id: Some("T001".to_string()),
                trigger: SlackTriggerType::AppMention,
                channel: "C001".to_string(),
                user: "U001".to_string(),
                text: text.unwrap_or("monitor alert").to_string(),
                ts: ts.to_string(),
                thread_ts: thread_ts.map(ToString::to_string),
            },
        }
    }

    struct ExecutionFixtures {
        deps: InvestigationExecutionDeps,
        slack_reply_calls: Arc<Mutex<Vec<SlackThreadReplyInput>>>,
        slack_thread_history_calls: Arc<Mutex<Vec<FetchSlackThreadHistoryInput>>>,
        investigation_lead_runs: Arc<Mutex<Vec<CapturedInvestigationLeadRunInput>>>,
    }

    fn create_execution_fixtures(
        thread_history_response: Option<Result<Vec<SlackThreadMessage>, PortError>>,
    ) -> ExecutionFixtures {
        let slack_reply_calls = Arc::new(Mutex::new(Vec::new()));
        let slack_thread_history_calls = Arc::new(Mutex::new(Vec::new()));
        let investigation_lead_runs = Arc::new(Mutex::new(Vec::new()));

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

        let mut investigation_lead_runner = MockInvestigationLeadRunnerPort::new();
        let captured_runs = Arc::clone(&investigation_lead_runs);
        investigation_lead_runner.expect_run().times(1).returning(
            move |input: RunInvestigationLeadInput| {
                captured_runs.lock().expect("lock captured runs").push(
                    CapturedInvestigationLeadRunInput {
                        alert_context: input.alert_context,
                        runtime: input.context.runtime,
                    },
                );

                Ok(InvestigationLeadRunReport {
                    result_text: "investigation_lead result".to_string(),
                    usage: USAGE_SNAPSHOT,
                    execution: LlmExecutionMetadata {
                        provider: "openai".to_string(),
                        model: "gpt-test".to_string(),
                    },
                })
            },
        );

        let deps = InvestigationExecutionDeps {
            slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            investigation_progress_session_factory_port: create_progress_session_factory(),
            slack_thread_history_port: Arc::new(slack_thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            investigation_resources: create_resources(),
            investigation_lead_runner: Arc::new(investigation_lead_runner)
                as Arc<dyn InvestigationLeadRunnerPort>,
            logger: Arc::new(NoopLogger) as Arc<dyn InvestigationLogger>,
        };

        ExecutionFixtures {
            deps,
            slack_reply_calls,
            slack_thread_history_calls,
            investigation_lead_runs,
        }
    }

    #[tokio::test]
    async fn fetches_thread_history_only_for_thread_replies() {
        let fixtures = create_execution_fixtures(Some(Ok(vec![SlackThreadMessage {
            ts: "1710000000.000001".to_string(),
            user: Some("U999".to_string()),
            text: "thread context".to_string(),
        }])));
        let ExecutionFixtures {
            deps,
            slack_reply_calls,
            slack_thread_history_calls,
            investigation_lead_runs,
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_id: "job-1".to_string(),
            retry_count: 0,
            payload: create_payload(
                "1710000000.000002",
                Some("1710000000.000001"),
                Some("<@U999> monitor alert"),
            ),
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

        let captured = investigation_lead_runs
            .lock()
            .expect("lock captured runs")
            .clone();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].alert_context.trigger_message_text,
            "<@U999> monitor alert"
        );
        assert_eq!(
            captured[0].alert_context.thread_transcript,
            "[ts: 1710000000.000001 | iso: 2024-03-09T16:00:00.000Z] U999 (You): thread context"
        );
        assert_eq!(
            slack_reply_calls.lock().expect("lock reply calls").clone(),
            vec![SlackThreadReplyInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                text: "investigation_lead result".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn does_not_fetch_thread_history_for_non_thread_messages() {
        let fixtures = create_execution_fixtures(None);
        let ExecutionFixtures {
            deps,
            slack_thread_history_calls,
            investigation_lead_runs,
            ..
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_id: "job-2".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000100", None, None),
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

        let captured = investigation_lead_runs
            .lock()
            .expect("lock captured runs")
            .clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].alert_context.thread_transcript.is_empty());
    }

    #[tokio::test]
    async fn falls_back_when_thread_history_fetch_fails() {
        let fixtures = create_execution_fixtures(Some(Err(PortError::new("slack api failed"))));
        let ExecutionFixtures {
            deps,
            investigation_lead_runs,
            ..
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_id: "job-3".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000200", Some("1710000000.000150"), None),
            deps,
        })
        .await;

        assert!(result.is_ok());

        let captured = investigation_lead_runs
            .lock()
            .expect("lock captured runs")
            .clone();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].alert_context.thread_transcript.is_empty());
    }

    #[test]
    fn token_log_meta_omits_investigation_lead_total_tokens() {
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
        assert!(!meta.contains_key("llm_tokens_total_investigation_lead"));
    }
}
