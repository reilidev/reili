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
    use super::InvestigationLogMeta;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::{AgentRunFailedError, PortError};
    use reili_core::investigation::{
        AlertContext, CompleteInvestigationProgressSessionInput, InvestigationJobPayload,
        InvestigationLeadRunnerPort, InvestigationProgressSessionFactoryPort,
        InvestigationProgressSessionPort, InvestigationResources, InvestigationRuntime,
        LlmUsageSnapshot, RunInvestigationLeadInput, StartInvestigationProgressSessionInput,
    };
    use reili_core::knowledge::{WebSearchInput, WebSearchPort, WebSearchResult};
    use reili_core::logger::{LogEntry, LogLevel};
    use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage, SlackTriggerType};
    use reili_core::messaging::slack::{
        SlackThreadHistoryPort, SlackThreadReplyInput, SlackThreadReplyPort,
    };
    use reili_core::monitoring::datadog::{
        DatadogEventSearchParams, DatadogEventSearchPort, DatadogEventSearchResult,
        DatadogLogAggregateBucket, DatadogLogAggregateParams, DatadogLogAggregatePort,
        DatadogLogSearchParams, DatadogLogSearchPort, DatadogLogSearchResult,
        DatadogMetricCatalogParams, DatadogMetricCatalogPort, DatadogMetricQueryParams,
        DatadogMetricQueryPort, DatadogMetricQueryResult,
    };
    use reili_core::source_code::github::{
        GithubCodeSearchPort, GithubCodeSearchResultItem, GithubIssueSearchResultItem,
        GithubPullRequestDiff, GithubPullRequestParams, GithubPullRequestPort,
        GithubPullRequestSummary, GithubRepoSearchResultItem, GithubRepositoryContent,
        GithubRepositoryContentParams, GithubRepositoryContentPort, GithubSearchParams,
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

    #[derive(Default)]
    struct MockSlackReplyPort {
        calls: Mutex<Vec<SlackThreadReplyInput>>,
    }

    #[async_trait]
    impl SlackThreadReplyPort for MockSlackReplyPort {
        async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError> {
            self.calls.lock().expect("lock reply calls").push(input);
            Ok(())
        }
    }

    struct MockInvestigationProgressSessionFactoryPort;

    struct MockInvestigationProgressSessionPort;

    #[async_trait]
    impl InvestigationProgressSessionPort for MockInvestigationProgressSessionPort {
        async fn start(&mut self) {}

        async fn apply(&mut self, _update: reili_core::investigation::InvestigationProgressUpdate) {
        }

        async fn complete(&mut self, _input: CompleteInvestigationProgressSessionInput) {}
    }

    impl InvestigationProgressSessionFactoryPort for MockInvestigationProgressSessionFactoryPort {
        fn create_for_thread(
            &self,
            _input: StartInvestigationProgressSessionInput,
        ) -> Box<dyn InvestigationProgressSessionPort> {
            Box::new(MockInvestigationProgressSessionPort)
        }
    }

    struct MockSlackThreadHistoryPort {
        response: Mutex<Result<Vec<SlackThreadMessage>, PortError>>,
        calls: Mutex<Vec<reili_core::messaging::slack::FetchSlackThreadHistoryInput>>,
    }

    impl MockSlackThreadHistoryPort {
        fn success(messages: Vec<SlackThreadMessage>) -> Self {
            Self {
                response: Mutex::new(Ok(messages)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                response: Mutex::new(Err(PortError::new(message))),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<reili_core::messaging::slack::FetchSlackThreadHistoryInput> {
            self.calls.lock().expect("lock history calls").clone()
        }
    }

    #[async_trait]
    impl SlackThreadHistoryPort for MockSlackThreadHistoryPort {
        async fn fetch_thread_history(
            &self,
            input: reili_core::messaging::slack::FetchSlackThreadHistoryInput,
        ) -> Result<Vec<SlackThreadMessage>, PortError> {
            self.calls.lock().expect("lock history calls").push(input);
            self.response.lock().expect("lock history response").clone()
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedInvestigationLeadRunInput {
        alert_context: AlertContext,
        runtime: InvestigationRuntime,
    }

    struct MockInvestigationLeadRunner {
        captured: Mutex<Vec<CapturedInvestigationLeadRunInput>>,
    }

    impl MockInvestigationLeadRunner {
        fn new() -> Self {
            Self {
                captured: Mutex::new(Vec::new()),
            }
        }

        fn captured(&self) -> Vec<CapturedInvestigationLeadRunInput> {
            self.captured.lock().expect("lock captured runs").clone()
        }
    }

    #[async_trait]
    impl InvestigationLeadRunnerPort for MockInvestigationLeadRunner {
        async fn run(
            &self,
            input: RunInvestigationLeadInput,
        ) -> Result<reili_core::investigation::InvestigationLeadRunReport, AgentRunFailedError>
        {
            self.captured.lock().expect("lock captured runs").push(
                CapturedInvestigationLeadRunInput {
                    alert_context: input.alert_context,
                    runtime: input.context.runtime,
                },
            );

            Ok(reili_core::investigation::InvestigationLeadRunReport {
                result_text: "investigation_lead result".to_string(),
                usage: USAGE_SNAPSHOT,
                execution: reili_core::investigation::LlmExecutionMetadata {
                    provider: "openai".to_string(),
                    model: "gpt-test".to_string(),
                },
            })
        }
    }

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
        warn_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
        error_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
    }

    impl InvestigationLogger for MockLogger {
        fn log(&self, entry: LogEntry) {
            match entry.level {
                LogLevel::Info => self
                    .info_logs
                    .lock()
                    .expect("lock info logs")
                    .push((entry.event.to_string(), entry.fields)),
                LogLevel::Warn => self
                    .warn_logs
                    .lock()
                    .expect("lock warn logs")
                    .push((entry.event.to_string(), entry.fields)),
                LogLevel::Error => self
                    .error_logs
                    .lock()
                    .expect("lock error logs")
                    .push((entry.event.to_string(), entry.fields)),
                LogLevel::Debug => {}
            }
        }
    }

    struct UnusedResourcesPort;

    #[async_trait]
    impl DatadogLogAggregatePort for UnusedResourcesPort {
        async fn aggregate_by_facet(
            &self,
            _params: DatadogLogAggregateParams,
        ) -> Result<Vec<DatadogLogAggregateBucket>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl DatadogLogSearchPort for UnusedResourcesPort {
        async fn search_logs(
            &self,
            _params: DatadogLogSearchParams,
        ) -> Result<Vec<DatadogLogSearchResult>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl DatadogMetricCatalogPort for UnusedResourcesPort {
        async fn list_metrics(
            &self,
            _params: DatadogMetricCatalogParams,
        ) -> Result<Vec<String>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl DatadogMetricQueryPort for UnusedResourcesPort {
        async fn query_metrics(
            &self,
            _params: DatadogMetricQueryParams,
        ) -> Result<Vec<DatadogMetricQueryResult>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl DatadogEventSearchPort for UnusedResourcesPort {
        async fn search_events(
            &self,
            _params: DatadogEventSearchParams,
        ) -> Result<Vec<DatadogEventSearchResult>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl GithubCodeSearchPort for UnusedResourcesPort {
        async fn search_code(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubCodeSearchResultItem>, PortError> {
            Err(PortError::new("unused"))
        }

        async fn search_repos(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubRepoSearchResultItem>, PortError> {
            Err(PortError::new("unused"))
        }

        async fn search_issues_and_pull_requests(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubIssueSearchResultItem>, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl GithubRepositoryContentPort for UnusedResourcesPort {
        async fn get_repository_content(
            &self,
            _params: GithubRepositoryContentParams,
        ) -> Result<GithubRepositoryContent, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl GithubPullRequestPort for UnusedResourcesPort {
        async fn get_pull_request(
            &self,
            _params: GithubPullRequestParams,
        ) -> Result<GithubPullRequestSummary, PortError> {
            Err(PortError::new("unused"))
        }

        async fn get_pull_request_diff(
            &self,
            _params: GithubPullRequestParams,
        ) -> Result<GithubPullRequestDiff, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl WebSearchPort for UnusedResourcesPort {
        async fn search(&self, _input: WebSearchInput) -> Result<WebSearchResult, PortError> {
            Err(PortError::new("unused"))
        }
    }

    fn create_resources() -> InvestigationResources {
        let port = Arc::new(UnusedResourcesPort);

        InvestigationResources {
            log_aggregate_port: Arc::clone(&port) as Arc<dyn DatadogLogAggregatePort>,
            log_search_port: Arc::clone(&port) as Arc<dyn DatadogLogSearchPort>,
            metric_catalog_port: Arc::clone(&port) as Arc<dyn DatadogMetricCatalogPort>,
            metric_query_port: Arc::clone(&port) as Arc<dyn DatadogMetricQueryPort>,
            event_search_port: Arc::clone(&port) as Arc<dyn DatadogEventSearchPort>,
            github_code_search_port: Arc::clone(&port) as Arc<dyn GithubCodeSearchPort>,
            github_repository_content_port: Arc::clone(&port)
                as Arc<dyn GithubRepositoryContentPort>,
            github_pull_request_port: Arc::clone(&port) as Arc<dyn GithubPullRequestPort>,
            web_search_port: Arc::clone(&port) as Arc<dyn WebSearchPort>,
        }
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
        slack_reply_port: Arc<MockSlackReplyPort>,
        slack_thread_history_port: Arc<MockSlackThreadHistoryPort>,
        investigation_lead_runner: Arc<MockInvestigationLeadRunner>,
    }

    fn create_execution_fixtures(
        slack_thread_history_port: Arc<MockSlackThreadHistoryPort>,
    ) -> ExecutionFixtures {
        let slack_reply_port = Arc::new(MockSlackReplyPort::default());
        let investigation_progress_session_factory_port =
            Arc::new(MockInvestigationProgressSessionFactoryPort);
        let investigation_lead_runner = Arc::new(MockInvestigationLeadRunner::new());
        let logger = Arc::new(MockLogger::default());

        let deps = InvestigationExecutionDeps {
            slack_reply_port: Arc::clone(&slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            investigation_progress_session_factory_port: Arc::clone(
                &investigation_progress_session_factory_port,
            )
                as Arc<dyn InvestigationProgressSessionFactoryPort>,
            slack_thread_history_port: Arc::clone(&slack_thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            investigation_resources: create_resources(),
            investigation_lead_runner: Arc::clone(&investigation_lead_runner)
                as Arc<dyn InvestigationLeadRunnerPort>,
            logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
        };

        ExecutionFixtures {
            deps,
            slack_reply_port,
            slack_thread_history_port,
            investigation_lead_runner,
        }
    }

    #[tokio::test]
    async fn fetches_thread_history_only_for_thread_replies() {
        let fixtures =
            create_execution_fixtures(Arc::new(MockSlackThreadHistoryPort::success(vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U999".to_string()),
                    text: "thread context".to_string(),
                },
            ])));
        let ExecutionFixtures {
            deps,
            slack_reply_port,
            slack_thread_history_port,
            investigation_lead_runner,
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
            slack_thread_history_port.calls(),
            vec![reili_core::messaging::slack::FetchSlackThreadHistoryInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
            }]
        );

        let captured = investigation_lead_runner.captured();
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
            slack_reply_port
                .calls
                .lock()
                .expect("lock reply calls")
                .clone(),
            vec![SlackThreadReplyInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                text: "investigation_lead result".to_string(),
            }]
        );
    }

    #[tokio::test]
    async fn does_not_fetch_thread_history_for_non_thread_messages() {
        let fixtures =
            create_execution_fixtures(Arc::new(MockSlackThreadHistoryPort::success(Vec::new())));
        let ExecutionFixtures {
            deps,
            slack_thread_history_port,
            investigation_lead_runner,
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
        assert!(slack_thread_history_port.calls().is_empty());

        let captured = investigation_lead_runner.captured();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].alert_context.thread_transcript.is_empty());
    }

    #[tokio::test]
    async fn falls_back_when_thread_history_fetch_fails() {
        let fixtures = create_execution_fixtures(Arc::new(MockSlackThreadHistoryPort::failure(
            "slack api failed",
        )));
        let ExecutionFixtures {
            deps,
            slack_thread_history_port: _,
            investigation_lead_runner,
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

        let captured = investigation_lead_runner.captured();
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
