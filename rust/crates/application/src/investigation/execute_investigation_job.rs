use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{
    CoordinatorRunReport, InvestigationContext, InvestigationCoordinatorRunnerPort,
    InvestigationProgressEventInput, InvestigationProgressEventPort, InvestigationResources,
    InvestigationRuntime, InvestigationSynthesizerRunnerPort, RunCoordinatorInput,
    RunSynthesizerInput, SYNTHESIZER_PROGRESS_OWNER_ID, SlackProgressStreamPort,
    SlackThreadHistoryPort, SlackThreadReplyInput, SlackThreadReplyPort, SynthesizerRunReport,
};
use sre_shared::types::{
    AlertContext, BuildInvestigationLlmTelemetryInput, InvestigationJobPayload,
    InvestigationJobType, InvestigationLlmTelemetry,
};
use tokio::sync::{Mutex, mpsc};

use super::execution_errors::{
    ExecuteInvestigationJobError, InvestigationExecutionFailedErrorInput,
    create_investigation_execution_failed_error, resolve_investigation_failure_error,
};
use super::logger::InvestigationLogger;
use super::services::{
    CoordinatorProgressEventHandler, CoordinatorProgressEventHandlerInput,
    CreateInvestigationProgressStreamSessionFactoryInput,
    CreateInvestigationProgressStreamSessionInput, InvestigationProgressReasoningInput,
    InvestigationProgressStreamSession, InvestigationProgressStreamSessionFactory,
    build_investigation_llm_telemetry, create_investigation_progress_stream_session_factory,
};
use super::slack_thread_context_loader::{
    SlackThreadContextLoader, SlackThreadContextLoaderDeps, SlackThreadContextLoaderInput,
    ThreadContextFetchFailedLogInput, ThreadContextLoaderLogger,
};
use crate::alert_intake::{ExtractAlertContextInput, extract_alert_context};

pub struct InvestigationExecutionDeps {
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub slack_progress_stream_port: Arc<dyn SlackProgressStreamPort>,
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub investigation_resources: InvestigationResources,
    pub coordinator_runner: Arc<dyn InvestigationCoordinatorRunnerPort>,
    pub synthesizer_runner: Arc<dyn InvestigationSynthesizerRunnerPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

pub struct ExecuteInvestigationJobInput {
    pub job_type: InvestigationJobType,
    pub job_id: String,
    pub retry_count: u32,
    pub payload: InvestigationJobPayload,
    pub deps: InvestigationExecutionDeps,
}

pub async fn execute_investigation_job(
    input: ExecuteInvestigationJobInput,
) -> Result<(), ExecuteInvestigationJobError> {
    let thread_ts = input
        .payload
        .message
        .thread_ts
        .clone()
        .unwrap_or_else(|| input.payload.message.ts.clone());
    let started_at = Instant::now();
    let started_at_iso = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

    let base_log_meta = BTreeMap::from([
        (
            "jobType".to_string(),
            investigation_job_type_to_string(&input.job_type),
        ),
        (
            "slackEventId".to_string(),
            input.payload.slack_event_id.clone(),
        ),
        ("jobId".to_string(), input.job_id.clone()),
        ("channel".to_string(), input.payload.message.channel.clone()),
        ("threadTs".to_string(), thread_ts.clone()),
        ("attempt".to_string(), (input.retry_count + 1).to_string()),
    ]);

    let progress_session_factory = create_investigation_progress_stream_session_factory(
        CreateInvestigationProgressStreamSessionFactoryInput {
            slack_stream_reply_port: Arc::clone(&input.deps.slack_progress_stream_port),
            reply_port: Arc::clone(&input.deps.slack_reply_port),
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
        CoordinatorProgressEventHandler::new(CoordinatorProgressEventHandlerInput {
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
        logger: Arc::new(ThreadContextLoggerAdapter {
            logger: Arc::clone(&input.deps.logger),
        }),
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
                success.synthesizer_report.report_text,
                success.llm_telemetry.clone(),
            )
            .await?;

            let duration_ms = started_at.elapsed().as_millis();
            let mut meta = merge_log_meta(
                &base_log_meta,
                &build_llm_token_log_meta(&success.llm_telemetry),
            );
            meta.insert(
                "worker_job_duration_ms".to_string(),
                duration_ms.to_string(),
            );
            meta.insert("latencyMs".to_string(), duration_ms.to_string());
            input.deps.logger.info("Processed investigation job", meta);
            Ok(())
        }
        Err(error) => {
            {
                let mut session = progress_session.lock().await;
                session.stop_as_failed().await;
            }

            let failure_error = resolve_investigation_failure_error(&error);
            let llm_telemetry =
                build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
                    coordinator_usage: failure_error.coordinator_usage,
                    synthesizer_usage: failure_error.synthesizer_usage,
                });

            let duration_ms = started_at.elapsed().as_millis();
            let mut meta =
                merge_log_meta(&base_log_meta, &build_llm_token_log_meta(&llm_telemetry));
            meta.insert(
                "worker_job_duration_ms".to_string(),
                duration_ms.to_string(),
            );
            meta.insert("latencyMs".to_string(), duration_ms.to_string());
            meta.insert("error".to_string(), failure_error.error_message);
            input.deps.logger.error("Failed investigation job", meta);
            Err(error)
        }
    }
}

struct InvestigationExecutionSuccess {
    synthesizer_report: SynthesizerRunReport,
    llm_telemetry: InvestigationLlmTelemetry,
}

async fn run_investigation(
    input: &ExecuteInvestigationJobInput,
    thread_ts: &str,
    started_at_iso: &str,
    base_log_meta: &BTreeMap<String, String>,
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
        resources: clone_investigation_resources(&input.deps.investigation_resources),
        runtime,
    };

    {
        let mut session = progress_session.lock().await;
        session.start().await;
    }

    let coordinator_report = input
        .deps
        .coordinator_runner
        .run(RunCoordinatorInput {
            alert_context: alert_context.clone(),
            context,
            on_progress_event: Arc::clone(&on_progress_event),
        })
        .await
        .map_err(ExecuteInvestigationJobError::from)?;

    {
        let mut session = progress_session.lock().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: SYNTHESIZER_PROGRESS_OWNER_ID.to_string(),
                title: "Reporting".to_string(),
                summary: String::new(),
            })
            .await;
    }

    let synthesizer_report = run_synthesis_stage(
        Arc::clone(&input.deps.synthesizer_runner),
        coordinator_report.clone(),
        alert_context,
        Arc::clone(&on_progress_event),
    )
    .await?;

    let llm_telemetry = build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
        coordinator_usage: coordinator_report.usage,
        synthesizer_usage: synthesizer_report.usage.clone(),
    });

    Ok(InvestigationExecutionSuccess {
        synthesizer_report,
        llm_telemetry,
    })
}

async fn run_synthesis_stage(
    synthesizer_runner: Arc<dyn InvestigationSynthesizerRunnerPort>,
    coordinator_report: CoordinatorRunReport,
    alert_context: AlertContext,
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
) -> Result<SynthesizerRunReport, ExecuteInvestigationJobError> {
    synthesizer_runner
        .run(RunSynthesizerInput {
            result: coordinator_report.result_text,
            alert_context,
            on_progress_event,
        })
        .await
        .map_err(|error| {
            let llm_telemetry =
                build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
                    coordinator_usage: coordinator_report.usage,
                    synthesizer_usage: error.usage.clone(),
                });
            ExecuteInvestigationJobError::InvestigationExecutionFailed(
                create_investigation_execution_failed_error(
                    InvestigationExecutionFailedErrorInput {
                        cause_message: error.cause_message,
                        llm_telemetry,
                    },
                ),
            )
        })
}

async fn post_slack_reply_stage(
    slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    channel: String,
    thread_ts: String,
    report_text: String,
    llm_telemetry: InvestigationLlmTelemetry,
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
                create_investigation_execution_failed_error(
                    InvestigationExecutionFailedErrorInput {
                        cause_message: error.message,
                        llm_telemetry,
                    },
                ),
            )
        })
}

fn build_llm_token_log_meta(telemetry: &InvestigationLlmTelemetry) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "llm_tokens_input_total".to_string(),
            telemetry.total.input_tokens.to_string(),
        ),
        (
            "llm_tokens_output_total".to_string(),
            telemetry.total.output_tokens.to_string(),
        ),
        (
            "llm_tokens_total".to_string(),
            telemetry.total.total_tokens.to_string(),
        ),
        (
            "llm_requests_total".to_string(),
            telemetry.total.requests.to_string(),
        ),
        (
            "llm_tokens_total_coordinator".to_string(),
            telemetry.coordinator.total_tokens.to_string(),
        ),
        (
            "llm_tokens_total_synthesizer".to_string(),
            telemetry.synthesizer.total_tokens.to_string(),
        ),
    ])
}

fn merge_log_meta(
    base: &BTreeMap<String, String>,
    append: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
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

pub(super) fn clone_investigation_resources(
    resources: &InvestigationResources,
) -> InvestigationResources {
    InvestigationResources {
        log_aggregate_port: Arc::clone(&resources.log_aggregate_port),
        log_search_port: Arc::clone(&resources.log_search_port),
        metric_catalog_port: Arc::clone(&resources.metric_catalog_port),
        metric_query_port: Arc::clone(&resources.metric_query_port),
        event_search_port: Arc::clone(&resources.event_search_port),
        datadog_site: resources.datadog_site.clone(),
        github_scope_org: resources.github_scope_org.clone(),
        github_search_port: Arc::clone(&resources.github_search_port),
    }
}

fn investigation_job_type_to_string(value: &InvestigationJobType) -> String {
    match value {
        InvestigationJobType::AlertInvestigation => "alert_investigation".to_string(),
    }
}

struct ThreadContextLoggerAdapter {
    logger: Arc<dyn InvestigationLogger>,
}

impl ThreadContextLoaderLogger for ThreadContextLoggerAdapter {
    fn error(&self, message: &str, input: ThreadContextFetchFailedLogInput) {
        let mut meta = input.base_log_meta;
        meta.insert(
            "thread_context_fetch_latency_ms".to_string(),
            input.thread_context_fetch_latency_ms.to_string(),
        );
        meta.insert("error".to_string(), input.error);
        self.logger.error(message, meta);
    }
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
    handler: CoordinatorProgressEventHandler,
) {
    while let Some(event) = receiver.recv().await {
        handler.handle(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use sre_shared::errors::{CoordinatorRunFailedError, PortError, SynthesizerRunFailedError};
    use sre_shared::ports::outbound::{
        DatadogEventSearchParams, DatadogEventSearchPort, DatadogEventSearchResult,
        DatadogLogAggregateBucket, DatadogLogAggregateParams, DatadogLogAggregatePort,
        DatadogLogSearchParams, DatadogLogSearchPort, DatadogLogSearchResult,
        DatadogMetricCatalogParams, DatadogMetricCatalogPort, DatadogMetricQueryParams,
        DatadogMetricQueryPort, DatadogMetricQueryResult, GithubCodeSearchResultItem,
        GithubIssueSearchResultItem, GithubPullRequestDiff, GithubPullRequestParams,
        GithubPullRequestSummary, GithubRepoSearchResultItem, GithubRepositoryContent,
        GithubRepositoryContentParams, GithubSearchParams, GithubSearchPort,
        InvestigationCoordinatorRunnerPort, InvestigationResources, InvestigationRuntime,
        InvestigationSynthesizerRunnerPort, RunCoordinatorInput, RunSynthesizerInput,
        SlackProgressStreamPort, SlackThreadHistoryPort, SlackThreadReplyInput,
        SlackThreadReplyPort, StartSlackProgressStreamInput, StartSlackProgressStreamOutput,
        SynthesizerRunReport,
    };
    use sre_shared::types::{
        AlertContext, InvestigationJobPayload, InvestigationJobType, LlmUsageSnapshot,
        SlackMessage, SlackThreadMessage, SlackTriggerType,
    };

    use super::{
        ExecuteInvestigationJobInput, InvestigationExecutionDeps, execute_investigation_job,
    };
    use crate::investigation::logger::InvestigationLogger;

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

    struct MockSlackProgressStreamPort;

    #[async_trait]
    impl SlackProgressStreamPort for MockSlackProgressStreamPort {
        async fn start(
            &self,
            _input: StartSlackProgressStreamInput,
        ) -> Result<StartSlackProgressStreamOutput, PortError> {
            Ok(StartSlackProgressStreamOutput {
                stream_ts: "stream-1".to_string(),
            })
        }

        async fn append(
            &self,
            _input: sre_shared::ports::outbound::AppendSlackProgressStreamInput,
        ) -> Result<(), PortError> {
            Ok(())
        }

        async fn stop(
            &self,
            _input: sre_shared::ports::outbound::StopSlackProgressStreamInput,
        ) -> Result<(), PortError> {
            Ok(())
        }
    }

    struct MockSlackThreadHistoryPort {
        response: Mutex<Result<Vec<SlackThreadMessage>, PortError>>,
        calls: Mutex<Vec<sre_shared::ports::outbound::FetchSlackThreadHistoryInput>>,
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

        fn calls(&self) -> Vec<sre_shared::ports::outbound::FetchSlackThreadHistoryInput> {
            self.calls.lock().expect("lock history calls").clone()
        }
    }

    #[async_trait]
    impl SlackThreadHistoryPort for MockSlackThreadHistoryPort {
        async fn fetch_thread_history(
            &self,
            input: sre_shared::ports::outbound::FetchSlackThreadHistoryInput,
        ) -> Result<Vec<SlackThreadMessage>, PortError> {
            self.calls.lock().expect("lock history calls").push(input);
            self.response.lock().expect("lock history response").clone()
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CapturedCoordinatorRunInput {
        alert_context: AlertContext,
        runtime: InvestigationRuntime,
    }

    struct MockCoordinatorRunner {
        captured: Mutex<Vec<CapturedCoordinatorRunInput>>,
    }

    impl MockCoordinatorRunner {
        fn new() -> Self {
            Self {
                captured: Mutex::new(Vec::new()),
            }
        }

        fn captured(&self) -> Vec<CapturedCoordinatorRunInput> {
            self.captured.lock().expect("lock captured runs").clone()
        }
    }

    #[async_trait]
    impl InvestigationCoordinatorRunnerPort for MockCoordinatorRunner {
        async fn run(
            &self,
            input: RunCoordinatorInput,
        ) -> Result<sre_shared::ports::outbound::CoordinatorRunReport, CoordinatorRunFailedError>
        {
            self.captured
                .lock()
                .expect("lock captured runs")
                .push(CapturedCoordinatorRunInput {
                    alert_context: input.alert_context,
                    runtime: input.context.runtime,
                });

            Ok(sre_shared::ports::outbound::CoordinatorRunReport {
                result_text: "coordinator result".to_string(),
                usage: USAGE_SNAPSHOT,
            })
        }
    }

    struct MockSynthesizerRunner;

    #[async_trait]
    impl InvestigationSynthesizerRunnerPort for MockSynthesizerRunner {
        async fn run(
            &self,
            _input: RunSynthesizerInput,
        ) -> Result<SynthesizerRunReport, SynthesizerRunFailedError> {
            Ok(SynthesizerRunReport {
                report_text: "final report".to_string(),
                usage: USAGE_SNAPSHOT,
            })
        }
    }

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
        warn_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
        error_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
    }

    impl InvestigationLogger for MockLogger {
        fn info(&self, message: &str, meta: BTreeMap<String, String>) {
            self.info_logs
                .lock()
                .expect("lock info logs")
                .push((message.to_string(), meta));
        }

        fn warn(&self, message: &str, meta: BTreeMap<String, String>) {
            self.warn_logs
                .lock()
                .expect("lock warn logs")
                .push((message.to_string(), meta));
        }

        fn error(&self, message: &str, meta: BTreeMap<String, String>) {
            self.error_logs
                .lock()
                .expect("lock error logs")
                .push((message.to_string(), meta));
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
    impl GithubSearchPort for UnusedResourcesPort {
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

        async fn get_repository_content(
            &self,
            _params: GithubRepositoryContentParams,
        ) -> Result<GithubRepositoryContent, PortError> {
            Err(PortError::new("unused"))
        }

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

    fn create_resources() -> InvestigationResources {
        let port = Arc::new(UnusedResourcesPort);

        InvestigationResources {
            log_aggregate_port: Arc::clone(&port) as Arc<dyn DatadogLogAggregatePort>,
            log_search_port: Arc::clone(&port) as Arc<dyn DatadogLogSearchPort>,
            metric_catalog_port: Arc::clone(&port) as Arc<dyn DatadogMetricCatalogPort>,
            metric_query_port: Arc::clone(&port) as Arc<dyn DatadogMetricQueryPort>,
            event_search_port: Arc::clone(&port) as Arc<dyn DatadogEventSearchPort>,
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            github_search_port: Arc::clone(&port) as Arc<dyn GithubSearchPort>,
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
        slack_thread_history_port: Arc<MockSlackThreadHistoryPort>,
        coordinator_runner: Arc<MockCoordinatorRunner>,
    }

    fn create_execution_fixtures(
        slack_thread_history_port: Arc<MockSlackThreadHistoryPort>,
    ) -> ExecutionFixtures {
        let slack_reply_port = Arc::new(MockSlackReplyPort::default());
        let slack_progress_stream_port = Arc::new(MockSlackProgressStreamPort);
        let coordinator_runner = Arc::new(MockCoordinatorRunner::new());
        let synthesizer_runner = Arc::new(MockSynthesizerRunner);
        let logger = Arc::new(MockLogger::default());

        let deps = InvestigationExecutionDeps {
            slack_reply_port: Arc::clone(&slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            slack_progress_stream_port: Arc::clone(&slack_progress_stream_port)
                as Arc<dyn SlackProgressStreamPort>,
            slack_thread_history_port: Arc::clone(&slack_thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            investigation_resources: create_resources(),
            coordinator_runner: Arc::clone(&coordinator_runner)
                as Arc<dyn InvestigationCoordinatorRunnerPort>,
            synthesizer_runner: Arc::clone(&synthesizer_runner)
                as Arc<dyn InvestigationSynthesizerRunnerPort>,
            logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
        };

        ExecutionFixtures {
            deps,
            slack_thread_history_port,
            coordinator_runner,
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
            slack_thread_history_port,
            coordinator_runner,
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_type: InvestigationJobType::AlertInvestigation,
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
            vec![sre_shared::ports::outbound::FetchSlackThreadHistoryInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
            }]
        );

        let captured = coordinator_runner.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].alert_context.trigger_message_text,
            "<@U999> monitor alert"
        );
        assert_eq!(
            captured[0].alert_context.thread_transcript,
            "[ts: 1710000000.000001 | iso: 2024-03-09T16:00:00.000Z] U999 (You): thread context"
        );
    }

    #[tokio::test]
    async fn does_not_fetch_thread_history_for_non_thread_messages() {
        let fixtures =
            create_execution_fixtures(Arc::new(MockSlackThreadHistoryPort::success(Vec::new())));
        let ExecutionFixtures {
            deps,
            slack_thread_history_port,
            coordinator_runner,
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_type: InvestigationJobType::AlertInvestigation,
            job_id: "job-2".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000100", None, None),
            deps,
        })
        .await;

        assert!(result.is_ok());
        assert!(slack_thread_history_port.calls().is_empty());

        let captured = coordinator_runner.captured();
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
            coordinator_runner,
        } = fixtures;

        let result = execute_investigation_job(ExecuteInvestigationJobInput {
            job_type: InvestigationJobType::AlertInvestigation,
            job_id: "job-3".to_string(),
            retry_count: 0,
            payload: create_payload("1710000000.000200", Some("1710000000.000150"), None),
            deps,
        })
        .await;

        assert!(result.is_ok());

        let captured = coordinator_runner.captured();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].alert_context.thread_transcript.is_empty());
    }
}
