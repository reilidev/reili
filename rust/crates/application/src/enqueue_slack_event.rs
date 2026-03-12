use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use sre_shared::errors::PortError;
use sre_shared::ports::inbound::SlackMessageHandlerPort;
use sre_shared::ports::outbound::{
    SlackThreadReplyInput, SlackThreadReplyPort, WorkerJobDispatcherPort,
};
use sre_shared::types::{InvestigationJob, InvestigationJobPayload, SlackMessage};
use tokio::time::sleep;
use uuid::Uuid;

use crate::investigation::InvestigationLogger;

pub struct EnqueueSlackEventUseCaseDeps {
    pub worker_job_dispatcher: Arc<dyn WorkerJobDispatcherPort>,
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub logger: Arc<dyn InvestigationLogger>,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
}

pub struct EnqueueSlackEventUseCase {
    deps: EnqueueSlackEventUseCaseDeps,
}

impl EnqueueSlackEventUseCase {
    pub fn new(deps: EnqueueSlackEventUseCaseDeps) -> Self {
        Self { deps }
    }

    async fn dispatch_with_retry(&self, job: &InvestigationJob) -> Result<(), PortError> {
        let max_attempts = self.deps.job_max_retry.saturating_add(1);
        let mut attempt = 0_u32;

        while attempt < max_attempts {
            attempt += 1;

            match self.deps.worker_job_dispatcher.dispatch(job.clone()).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if attempt >= max_attempts {
                        return Err(error);
                    }

                    self.deps.logger.warn(
                        "Retrying worker dispatch",
                        retry_log_meta(RetryLogMetaInput {
                            job,
                            attempt,
                            max_attempts,
                            error: &error,
                        }),
                    );

                    sleep(Duration::from_millis(self.deps.job_backoff_ms)).await;
                }
            }
        }

        Err(PortError::new(
            "Failed to dispatch worker job after retries",
        ))
    }
}

#[async_trait]
impl SlackMessageHandlerPort for EnqueueSlackEventUseCase {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        let event_started_at = Instant::now();
        let thread_ts = message.thread_ts_or_ts().to_string();
        let job = build_investigation_job(BuildInvestigationJobInput {
            message: message.clone(),
            received_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        });

        let dispatch_started_at = Instant::now();
        match self.dispatch_with_retry(&job).await {
            Ok(()) => {
                self.deps.logger.info(
                    "Dispatched worker job",
                    BTreeMap::from([
                        ("slackEventId".to_string(), message.slack_event_id),
                        ("jobId".to_string(), job.job_id),
                        ("channel".to_string(), message.channel),
                        ("threadTs".to_string(), thread_ts),
                        (
                            "ingress_dispatch_latency_ms".to_string(),
                            dispatch_started_at.elapsed().as_millis().to_string(),
                        ),
                        (
                            "ingress_ack_latency_ms".to_string(),
                            event_started_at.elapsed().as_millis().to_string(),
                        ),
                    ]),
                );

                Ok(())
            }
            Err(dispatch_error) => {
                self.deps.logger.error(
                    "Failed to dispatch worker job",
                    BTreeMap::from([
                        ("slackEventId".to_string(), message.slack_event_id),
                        ("jobId".to_string(), job.job_id),
                        ("channel".to_string(), message.channel.clone()),
                        ("threadTs".to_string(), thread_ts.clone()),
                        ("error".to_string(), dispatch_error.message.clone()),
                        (
                            "ingress_ack_latency_ms".to_string(),
                            event_started_at.elapsed().as_millis().to_string(),
                        ),
                    ]),
                );

                self.deps
                    .slack_reply_port
                    .post_thread_reply(SlackThreadReplyInput {
                        channel: message.channel,
                        thread_ts,
                        text: format!("Failed to queue investigation: {}", dispatch_error.message),
                    })
                    .await
            }
        }
    }
}

struct BuildInvestigationJobInput {
    message: SlackMessage,
    received_at: String,
}

fn build_investigation_job(input: BuildInvestigationJobInput) -> InvestigationJob {
    let slack_event_id = input.message.slack_event_id.clone();

    InvestigationJob {
        job_id: Uuid::new_v4().to_string(),
        received_at: input.received_at,
        payload: InvestigationJobPayload {
            slack_event_id,
            message: input.message,
        },
        retry_count: 0,
    }
}

struct RetryLogMetaInput<'a> {
    job: &'a InvestigationJob,
    attempt: u32,
    max_attempts: u32,
    error: &'a PortError,
}

fn retry_log_meta(input: RetryLogMetaInput<'_>) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "slackEventId".to_string(),
            input.job.payload.slack_event_id.clone(),
        ),
        ("jobId".to_string(), input.job.job_id.clone()),
        ("attempt".to_string(), input.attempt.to_string()),
        (
            "remainingAttempts".to_string(),
            input.max_attempts.saturating_sub(input.attempt).to_string(),
        ),
        ("error".to_string(), input.error.message.clone()),
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, InvestigationLogger, PortError,
        SlackMessage, SlackMessageHandlerPort, SlackThreadReplyInput, SlackThreadReplyPort,
        WorkerJobDispatcherPort,
    };
    use async_trait::async_trait;
    use sre_shared::types::SlackTriggerType;
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use sre_shared::types::InvestigationJob;

    #[derive(Debug, Clone)]
    struct LogEntry {
        message: String,
        meta: BTreeMap<String, String>,
    }

    #[derive(Default)]
    struct MockWorkerJobDispatcher {
        dispatched_jobs: Mutex<Vec<InvestigationJob>>,
        dispatch_results: Mutex<VecDeque<Result<(), PortError>>>,
    }

    impl MockWorkerJobDispatcher {
        fn dispatched_jobs(&self) -> Vec<InvestigationJob> {
            self.dispatched_jobs
                .lock()
                .expect("lock dispatched jobs")
                .clone()
        }

        fn with_dispatch_results(&self, results: Vec<Result<(), PortError>>) {
            let mut lock = self.dispatch_results.lock().expect("lock dispatch results");
            *lock = VecDeque::from(results);
        }
    }

    #[async_trait]
    impl WorkerJobDispatcherPort for MockWorkerJobDispatcher {
        async fn dispatch(&self, job: InvestigationJob) -> Result<(), PortError> {
            self.dispatched_jobs
                .lock()
                .expect("lock dispatched jobs")
                .push(job);

            let mut lock = self.dispatch_results.lock().expect("lock dispatch results");
            match lock.pop_front() {
                Some(result) => result,
                None => Ok(()),
            }
        }
    }

    #[derive(Default)]
    struct MockSlackThreadReplyPort {
        posted_replies: Mutex<Vec<SlackThreadReplyInput>>,
        reply_results: Mutex<VecDeque<Result<(), PortError>>>,
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
            let mut lock = self.reply_results.lock().expect("lock reply results");
            match lock.pop_front() {
                Some(result) => result,
                None => Ok(()),
            }
        }
    }

    #[derive(Default)]
    struct MockLogger {
        infos: Mutex<Vec<LogEntry>>,
        warns: Mutex<Vec<LogEntry>>,
        errors: Mutex<Vec<LogEntry>>,
    }

    impl MockLogger {
        fn warns(&self) -> Vec<LogEntry> {
            self.warns.lock().expect("lock warns").clone()
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

        fn warn(&self, message: &str, meta: BTreeMap<String, String>) {
            self.warns.lock().expect("lock warns").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }

        fn error(&self, message: &str, meta: BTreeMap<String, String>) {
            self.errors.lock().expect("lock errors").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }
    }

    struct TestContext {
        use_case: EnqueueSlackEventUseCase,
        worker_job_dispatcher: Arc<MockWorkerJobDispatcher>,
        slack_reply_port: Arc<MockSlackThreadReplyPort>,
        logger: Arc<MockLogger>,
    }

    fn create_use_case(input: CreateUseCaseInput) -> TestContext {
        let worker_job_dispatcher = Arc::new(MockWorkerJobDispatcher::default());
        worker_job_dispatcher.with_dispatch_results(input.dispatch_results);
        let slack_reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());

        let use_case = EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            worker_job_dispatcher: Arc::clone(&worker_job_dispatcher)
                as Arc<dyn WorkerJobDispatcherPort>,
            slack_reply_port: Arc::clone(&slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            job_max_retry: input.job_max_retry,
            job_backoff_ms: input.job_backoff_ms,
        });

        TestContext {
            use_case,
            worker_job_dispatcher,
            slack_reply_port,
            logger,
        }
    }

    struct CreateUseCaseInput {
        dispatch_results: Vec<Result<(), PortError>>,
        job_max_retry: u32,
        job_backoff_ms: u64,
    }

    fn create_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "high latency detected".to_string(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    #[tokio::test]
    async fn dispatches_alert_investigation_job() {
        let context = create_use_case(CreateUseCaseInput {
            dispatch_results: Vec::new(),
            job_max_retry: 0,
            job_backoff_ms: 0,
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let dispatched_jobs = context.worker_job_dispatcher.dispatched_jobs();
        assert_eq!(dispatched_jobs.len(), 1);
        assert_eq!(dispatched_jobs[0].retry_count, 0);
        assert_eq!(context.slack_reply_port.posted_replies().len(), 0);
    }

    #[tokio::test]
    async fn retries_dispatch_then_succeeds() {
        let context = create_use_case(CreateUseCaseInput {
            dispatch_results: vec![Err(PortError::new("temporary")), Ok(())],
            job_max_retry: 1,
            job_backoff_ms: 0,
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        assert_eq!(context.worker_job_dispatcher.dispatched_jobs().len(), 2);
        let warns = context.logger.warns();
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].message, "Retrying worker dispatch");
    }

    #[tokio::test]
    async fn posts_slack_reply_when_dispatch_exhausts_retries() {
        let context = create_use_case(CreateUseCaseInput {
            dispatch_results: vec![Err(PortError::new("fail-1")), Err(PortError::new("fail-2"))],
            job_max_retry: 1,
            job_backoff_ms: 0,
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let posted_replies = context.slack_reply_port.posted_replies();
        assert_eq!(posted_replies.len(), 1);
        assert_eq!(posted_replies[0].channel, "C001");
        assert_eq!(posted_replies[0].thread_ts, "1710000000.000001");
        assert_eq!(
            posted_replies[0].text,
            "Failed to queue investigation: fail-2"
        );

        let errors = context.logger.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "Failed to dispatch worker job");
        assert_eq!(errors[0].meta.get("error"), Some(&"fail-2".to_string()));
    }
}
