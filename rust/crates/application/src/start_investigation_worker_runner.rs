use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{
    CompleteJobInput, FailJobInput, InvestigationJobQueuePort, JobFailStatus,
    SlackThreadReplyInput, SlackThreadReplyPort,
};
use sre_shared::types::InvestigationJob;
use tokio::task::spawn;
use tokio::time::sleep;

use crate::investigation::{
    ExecuteInvestigationJobInput, InvestigationExecutionDeps, execute_investigation_job,
};

const IDLE_WAIT_MS: u64 = 150;

pub struct StartInvestigationWorkerRunnerUseCaseDeps {
    pub job_queue: Arc<InvestigationJobQueuePort>,
    pub investigation_execution_deps: InvestigationExecutionDeps,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
}

pub struct StartInvestigationWorkerRunnerUseCase {
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
    is_running: Arc<AtomicBool>,
}

impl StartInvestigationWorkerRunnerUseCase {
    pub fn new(deps: StartInvestigationWorkerRunnerUseCaseDeps) -> Self {
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
            spawn(run_investigation_worker_loop(WorkerLoopInput {
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
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
    is_running: Arc<AtomicBool>,
    worker_index: u32,
}

async fn run_investigation_worker_loop(input: WorkerLoopInput) {
    while input.is_running.load(Ordering::SeqCst) {
        run_worker_iteration(RunWorkerIterationInput {
            deps: Arc::clone(&input.deps),
            worker_index: input.worker_index,
        })
        .await;
    }
}

struct RunWorkerIterationInput {
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
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
            input.deps.investigation_execution_deps.logger.error(
                "Failed to claim worker job",
                BTreeMap::from([
                    ("workerIndex".to_string(), input.worker_index.to_string()),
                    ("error".to_string(), error.message),
                ]),
            );
            sleep(Duration::from_millis(IDLE_WAIT_MS)).await;
        }
    }
}

struct ProcessClaimedJobInput {
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
    worker_index: u32,
    job: InvestigationJob,
}

async fn process_claimed_job(input: ProcessClaimedJobInput) {
    let started_at = Instant::now();

    match execute_investigation_job(ExecuteInvestigationJobInput {
        job_type: input.job.job_type.clone(),
        job_id: input.job.job_id.clone(),
        retry_count: input.job.retry_count,
        payload: input.job.payload.clone(),
        deps: input.deps.investigation_execution_deps.clone(),
    })
    .await
    {
        Ok(()) => {
            match input
                .deps
                .job_queue
                .complete(CompleteJobInput {
                    job_id: input.job.job_id.clone(),
                })
                .await
            {
                Ok(()) => {
                    let queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
                        deps: Arc::clone(&input.deps),
                        worker_index: input.worker_index,
                    })
                    .await;

                    input.deps.investigation_execution_deps.logger.info(
                        "Completed worker job",
                        BTreeMap::from([
                            ("workerIndex".to_string(), input.worker_index.to_string()),
                            ("jobType".to_string(), input.job.job_type.to_string()),
                            (
                                "slackEventId".to_string(),
                                input.job.payload.slack_event_id.clone(),
                            ),
                            ("jobId".to_string(), input.job.job_id),
                            (
                                "channel".to_string(),
                                input.job.payload.message.channel.clone(),
                            ),
                            (
                                "threadTs".to_string(),
                                input.job.payload.message.thread_ts_or_ts().to_string(),
                            ),
                            (
                                "attempt".to_string(),
                                input.job.retry_count.saturating_add(1).to_string(),
                            ),
                            (
                                "worker_job_duration_ms".to_string(),
                                started_at.elapsed().as_millis().to_string(),
                            ),
                            ("worker_queue_depth".to_string(), queue_depth),
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
                    })
                    .await;
                }
            }
        }
        Err(error) => {
            handle_failed_claimed_job(HandleFailedClaimedJobInput {
                deps: Arc::clone(&input.deps),
                worker_index: input.worker_index,
                job: input.job,
                started_at,
                error_message: error.to_string(),
            })
            .await;
        }
    }
}

struct HandleFailedClaimedJobInput {
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
    worker_index: u32,
    job: InvestigationJob,
    started_at: Instant,
    error_message: String,
}

async fn handle_failed_claimed_job(input: HandleFailedClaimedJobInput) {
    let fail_result = input
        .deps
        .job_queue
        .fail(FailJobInput {
            job_id: input.job.job_id.clone(),
            reason: input.error_message.clone(),
            max_retry: input.deps.job_max_retry,
            backoff_ms: input.deps.job_backoff_ms,
        })
        .await;

    let fail_result = match fail_result {
        Ok(value) => value,
        Err(queue_fail_error) => {
            input.deps.investigation_execution_deps.logger.error(
                "Failed worker job",
                BTreeMap::from([
                    ("workerIndex".to_string(), input.worker_index.to_string()),
                    ("jobType".to_string(), input.job.job_type.to_string()),
                    (
                        "slackEventId".to_string(),
                        input.job.payload.slack_event_id.clone(),
                    ),
                    ("jobId".to_string(), input.job.job_id),
                    (
                        "channel".to_string(),
                        input.job.payload.message.channel.clone(),
                    ),
                    (
                        "threadTs".to_string(),
                        input.job.payload.message.thread_ts_or_ts().to_string(),
                    ),
                    (
                        "attempt".to_string(),
                        input.job.retry_count.saturating_add(1).to_string(),
                    ),
                    (
                        "worker_job_duration_ms".to_string(),
                        input.started_at.elapsed().as_millis().to_string(),
                    ),
                    ("status".to_string(), "queue_fail_error".to_string()),
                    ("error".to_string(), queue_fail_error.message),
                ]),
            );
            return;
        }
    };

    let queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
        deps: Arc::clone(&input.deps),
        worker_index: input.worker_index,
    })
    .await;

    input.deps.investigation_execution_deps.logger.error(
        "Failed worker job",
        BTreeMap::from([
            ("workerIndex".to_string(), input.worker_index.to_string()),
            ("jobType".to_string(), input.job.job_type.to_string()),
            (
                "slackEventId".to_string(),
                input.job.payload.slack_event_id.clone(),
            ),
            ("jobId".to_string(), input.job.job_id.clone()),
            (
                "channel".to_string(),
                input.job.payload.message.channel.clone(),
            ),
            (
                "threadTs".to_string(),
                input.job.payload.message.thread_ts_or_ts().to_string(),
            ),
            (
                "attempt".to_string(),
                input.job.retry_count.saturating_add(1).to_string(),
            ),
            (
                "worker_job_duration_ms".to_string(),
                input.started_at.elapsed().as_millis().to_string(),
            ),
            ("worker_queue_depth".to_string(), queue_depth),
            ("worker_job_failure_total".to_string(), "1".to_string()),
            (
                "status".to_string(),
                job_fail_status_to_string(&fail_result.status),
            ),
            ("error".to_string(), input.error_message.clone()),
        ]),
    );

    if fail_result.status == JobFailStatus::DeadLetter
        && let Err(dead_letter_error) =
            post_dead_letter_failure_message(PostDeadLetterFailureMessageInput {
                slack_reply_port: Arc::clone(
                    &input.deps.investigation_execution_deps.slack_reply_port,
                ),
                job: fail_result.job.clone(),
                error_message: input.error_message.clone(),
            })
            .await
    {
        input.deps.investigation_execution_deps.logger.error(
            "Failed dead-letter notification",
            BTreeMap::from([
                ("jobType".to_string(), fail_result.job.job_type.to_string()),
                (
                    "slackEventId".to_string(),
                    fail_result.job.payload.slack_event_id,
                ),
                ("jobId".to_string(), fail_result.job.job_id),
                (
                    "channel".to_string(),
                    fail_result.job.payload.message.channel.clone(),
                ),
                (
                    "threadTs".to_string(),
                    fail_result
                        .job
                        .payload
                        .message
                        .thread_ts_or_ts()
                        .to_string(),
                ),
                ("error".to_string(), dead_letter_error.message),
            ]),
        );
    }
}

struct ReadWorkerQueueDepthInput {
    deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
    worker_index: u32,
}

async fn read_worker_queue_depth(input: ReadWorkerQueueDepthInput) -> String {
    match input.deps.job_queue.get_depth().await {
        Ok(value) => value.to_string(),
        Err(error) => {
            input.deps.investigation_execution_deps.logger.error(
                "Failed to read worker queue depth",
                BTreeMap::from([
                    ("workerIndex".to_string(), input.worker_index.to_string()),
                    ("error".to_string(), error.message),
                ]),
            );
            "unknown".to_string()
        }
    }
}

struct PostDeadLetterFailureMessageInput {
    slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    job: InvestigationJob,
    error_message: String,
}

async fn post_dead_letter_failure_message(
    input: PostDeadLetterFailureMessageInput,
) -> Result<(), PortError> {
    input
        .slack_reply_port
        .post_thread_reply(SlackThreadReplyInput {
            channel: input.job.payload.message.channel.clone(),
            thread_ts: input.job.payload.message.thread_ts_or_ts().to_string(),
            text: format!(
                "Investigation failed after retries: {}",
                input.error_message
            ),
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
        Arc, BTreeMap, InvestigationExecutionDeps, InvestigationJob, InvestigationJobQueuePort,
        JobFailStatus, PortError, ProcessClaimedJobInput, SlackThreadReplyInput,
        SlackThreadReplyPort, StartInvestigationWorkerRunnerUseCaseDeps, handle_failed_claimed_job,
        process_claimed_job,
    };
    use async_trait::async_trait;
    use sre_shared::ports::outbound::{
        CompleteJobInput, DatadogEventSearchParams, DatadogEventSearchPort,
        DatadogEventSearchResult, DatadogLogAggregateBucket, DatadogLogAggregateParams,
        DatadogLogAggregatePort, DatadogLogSearchParams, DatadogLogSearchPort,
        DatadogLogSearchResult, DatadogMetricCatalogParams, DatadogMetricCatalogPort,
        DatadogMetricQueryParams, DatadogMetricQueryPort, DatadogMetricQueryResult, FailJobInput,
        GithubCodeSearchResultItem, GithubIssueSearchResultItem, GithubPullRequestDiff,
        GithubPullRequestParams, GithubPullRequestSummary, GithubRepoSearchResultItem,
        GithubRepositoryContent, GithubRepositoryContentParams, GithubSearchParams,
        GithubSearchPort, InvestigationCoordinatorRunnerPort, InvestigationResources,
        InvestigationSynthesizerRunnerPort, JobFailResult, JobQueuePort, RunCoordinatorInput,
        RunSynthesizerInput, SlackProgressStreamPort, SlackThreadHistoryPort,
        StartSlackProgressStreamInput, StartSlackProgressStreamOutput, SynthesizerRunReport,
        WebSearchInput, WebSearchPort, WebSearchResult,
    };
    use sre_shared::types::{
        InvestigationJobPayload, InvestigationJobType, LlmUsageSnapshot, SlackMessage,
        SlackThreadMessage, SlackTriggerType,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crate::investigation::InvestigationLogger;

    const USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
        requests: 1,
        input_tokens: 10,
        output_tokens: 20,
        total_tokens: 30,
    };

    #[derive(Debug, Clone)]
    struct LogEntry {
        message: String,
        meta: BTreeMap<String, String>,
    }

    #[derive(Default)]
    struct MockJobQueue {
        complete_inputs: Mutex<Vec<CompleteJobInput>>,
        fail_inputs: Mutex<Vec<FailJobInput>>,
        fail_results: Mutex<VecDeque<Result<JobFailResult<InvestigationJob>, PortError>>>,
        depth_results: Mutex<VecDeque<Result<usize, PortError>>>,
    }

    impl MockJobQueue {
        fn complete_inputs(&self) -> Vec<CompleteJobInput> {
            self.complete_inputs
                .lock()
                .expect("lock complete inputs")
                .clone()
        }

        fn fail_inputs(&self) -> Vec<FailJobInput> {
            self.fail_inputs.lock().expect("lock fail inputs").clone()
        }

        fn with_fail_results(
            &self,
            values: Vec<Result<JobFailResult<InvestigationJob>, PortError>>,
        ) {
            let mut lock = self.fail_results.lock().expect("lock fail results");
            *lock = VecDeque::from(values);
        }

        fn with_depth_results(&self, values: Vec<Result<usize, PortError>>) {
            let mut lock = self.depth_results.lock().expect("lock depth results");
            *lock = VecDeque::from(values);
        }
    }

    #[async_trait]
    impl JobQueuePort<InvestigationJob> for MockJobQueue {
        async fn enqueue(&self, _job: InvestigationJob) -> Result<(), PortError> {
            Ok(())
        }

        async fn claim(&self) -> Result<Option<InvestigationJob>, PortError> {
            Ok(None)
        }

        async fn complete(&self, input: CompleteJobInput) -> Result<(), PortError> {
            self.complete_inputs
                .lock()
                .expect("lock complete inputs")
                .push(input);
            Ok(())
        }

        async fn fail(
            &self,
            input: FailJobInput,
        ) -> Result<JobFailResult<InvestigationJob>, PortError> {
            self.fail_inputs
                .lock()
                .expect("lock fail inputs")
                .push(input);
            let mut lock = self.fail_results.lock().expect("lock fail results");
            match lock.pop_front() {
                Some(result) => result,
                None => Err(PortError::new("missing configured fail result")),
            }
        }

        async fn get_depth(&self) -> Result<usize, PortError> {
            let mut lock = self.depth_results.lock().expect("lock depth results");
            match lock.pop_front() {
                Some(result) => result,
                None => Ok(0),
            }
        }
    }

    #[derive(Default)]
    struct MockSlackThreadReplyPort {
        posted_replies: Mutex<Vec<SlackThreadReplyInput>>,
    }

    impl MockSlackThreadReplyPort {
        fn posted_replies(&self) -> Vec<SlackThreadReplyInput> {
            self.posted_replies
                .lock()
                .expect("lock posted replies")
                .clone()
        }
    }

    #[async_trait]
    impl SlackThreadReplyPort for MockSlackThreadReplyPort {
        async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError> {
            self.posted_replies
                .lock()
                .expect("lock posted replies")
                .push(input);
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

    struct MockSlackThreadHistoryPort;

    #[async_trait]
    impl SlackThreadHistoryPort for MockSlackThreadHistoryPort {
        async fn fetch_thread_history(
            &self,
            _input: sre_shared::ports::outbound::FetchSlackThreadHistoryInput,
        ) -> Result<Vec<SlackThreadMessage>, PortError> {
            Ok(Vec::new())
        }
    }

    struct MockCoordinatorRunner;

    #[async_trait]
    impl InvestigationCoordinatorRunnerPort for MockCoordinatorRunner {
        async fn run(
            &self,
            _input: RunCoordinatorInput,
        ) -> Result<
            sre_shared::ports::outbound::CoordinatorRunReport,
            sre_shared::errors::AgentRunFailedError,
        > {
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
        ) -> Result<SynthesizerRunReport, sre_shared::errors::AgentRunFailedError> {
            Ok(SynthesizerRunReport {
                report_text: "final report".to_string(),
                usage: USAGE_SNAPSHOT,
            })
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
        async fn search_repos(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubRepoSearchResultItem>, PortError> {
            Err(PortError::new("unused"))
        }

        async fn search_code(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubCodeSearchResultItem>, PortError> {
            Err(PortError::new("unused"))
        }

        async fn search_issues_and_pull_requests(
            &self,
            _params: GithubSearchParams,
        ) -> Result<Vec<GithubIssueSearchResultItem>, PortError> {
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

        async fn get_repository_content(
            &self,
            _params: GithubRepositoryContentParams,
        ) -> Result<GithubRepositoryContent, PortError> {
            Err(PortError::new("unused"))
        }
    }

    #[async_trait]
    impl WebSearchPort for UnusedResourcesPort {
        async fn search(&self, _input: WebSearchInput) -> Result<WebSearchResult, PortError> {
            Err(PortError::new("unused"))
        }
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

    impl InvestigationLogger for MockLogger {
        fn info(&self, message: &str, meta: BTreeMap<String, String>) {
            self.infos.lock().expect("lock infos").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }

        fn warn(&self, _message: &str, _meta: BTreeMap<String, String>) {}

        fn error(&self, message: &str, meta: BTreeMap<String, String>) {
            self.errors.lock().expect("lock errors").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }
    }

    struct TestContext {
        deps: Arc<StartInvestigationWorkerRunnerUseCaseDeps>,
        job_queue: Arc<MockJobQueue>,
        slack_reply_port: Arc<MockSlackThreadReplyPort>,
        logger: Arc<MockLogger>,
    }

    fn create_context() -> TestContext {
        let job_queue = Arc::new(MockJobQueue::default());
        let slack_reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());
        let resources_port = Arc::new(UnusedResourcesPort);
        let deps = Arc::new(StartInvestigationWorkerRunnerUseCaseDeps {
            job_queue: Arc::clone(&job_queue) as Arc<InvestigationJobQueuePort>,
            investigation_execution_deps: InvestigationExecutionDeps {
                slack_reply_port: Arc::clone(&slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
                slack_progress_stream_port: Arc::new(MockSlackProgressStreamPort),
                slack_thread_history_port: Arc::new(MockSlackThreadHistoryPort),
                investigation_resources: InvestigationResources {
                    log_aggregate_port: Arc::clone(&resources_port)
                        as Arc<dyn DatadogLogAggregatePort>,
                    log_search_port: Arc::clone(&resources_port) as Arc<dyn DatadogLogSearchPort>,
                    metric_catalog_port: Arc::clone(&resources_port)
                        as Arc<dyn DatadogMetricCatalogPort>,
                    metric_query_port: Arc::clone(&resources_port)
                        as Arc<dyn DatadogMetricQueryPort>,
                    event_search_port: Arc::clone(&resources_port)
                        as Arc<dyn DatadogEventSearchPort>,
                    datadog_site: "datadoghq.com".to_string(),
                    github_scope_org: "acme".to_string(),
                    github_search_port: Arc::clone(&resources_port) as Arc<dyn GithubSearchPort>,
                    web_search_port: resources_port as Arc<dyn WebSearchPort>,
                },
                coordinator_runner: Arc::new(MockCoordinatorRunner),
                synthesizer_runner: Arc::new(MockSynthesizerRunner),
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
            worker_concurrency: 1,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
        });

        TestContext {
            deps,
            job_queue,
            slack_reply_port,
            logger,
        }
    }

    fn create_job(input: CreateJobInput) -> InvestigationJob {
        InvestigationJob {
            job_id: input.job_id,
            job_type: InvestigationJobType::AlertInvestigation,
            received_at: "2026-03-04T00:00:00.000Z".to_string(),
            payload: InvestigationJobPayload {
                slack_event_id: "Ev001".to_string(),
                message: SlackMessage {
                    slack_event_id: "Ev001".to_string(),
                    team_id: Some("T001".to_string()),
                    trigger: SlackTriggerType::Message,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    text: "alert".to_string(),
                    ts: "1710000000.000001".to_string(),
                    thread_ts: input.thread_ts,
                },
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
        let context = create_context();
        context.job_queue.with_depth_results(vec![Ok(3)]);

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

        assert_eq!(context.job_queue.complete_inputs().len(), 1);
        assert_eq!(context.job_queue.fail_inputs().len(), 0);
        assert_eq!(context.slack_reply_port.posted_replies().len(), 1);
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
                .and_then(|entry| entry.meta.get("worker_queue_depth")),
            Some(&"3".to_string())
        );
    }

    #[tokio::test]
    async fn handle_failed_claimed_job_requeues_and_logs() {
        let context = create_context();
        context.job_queue.with_fail_results(vec![Ok(JobFailResult {
            status: JobFailStatus::Requeued,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 1,
                thread_ts: None,
            }),
        })]);
        context.job_queue.with_depth_results(vec![Ok(2)]);

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
        })
        .await;

        let fail_inputs = context.job_queue.fail_inputs();
        assert_eq!(fail_inputs.len(), 1);
        assert_eq!(fail_inputs[0].reason, "processing failed");
        assert_eq!(fail_inputs[0].max_retry, 2);
        assert_eq!(fail_inputs[0].backoff_ms, 1_000);

        let error_logs = context.logger.errors();
        assert_eq!(error_logs.len(), 1);
        assert_eq!(error_logs[0].message, "Failed worker job");
        assert_eq!(
            error_logs[0].meta.get("status"),
            Some(&"requeued".to_string())
        );
        assert_eq!(context.slack_reply_port.posted_replies().len(), 0);
    }

    #[tokio::test]
    async fn handle_failed_claimed_job_posts_dead_letter_message() {
        let context = create_context();
        context.job_queue.with_fail_results(vec![Ok(JobFailResult {
            status: JobFailStatus::DeadLetter,
            job: create_job(CreateJobInput {
                job_id: "job-1".to_string(),
                retry_count: 2,
                thread_ts: Some("1710000000.000001".to_string()),
            }),
        })]);
        context.job_queue.with_depth_results(vec![Ok(0)]);

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
        })
        .await;

        let posted_replies = context.slack_reply_port.posted_replies();
        assert_eq!(posted_replies.len(), 1);
        assert_eq!(
            posted_replies[0].text,
            "Investigation failed after retries: fatal failure"
        );
        assert_eq!(posted_replies[0].thread_ts, "1710000000.000001");
    }
}
