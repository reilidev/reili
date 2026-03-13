use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use sre_shared::errors::PortError;
use sre_shared::ports::inbound::SlackMessageHandlerPort;
use sre_shared::ports::outbound::{
    InvestigationJobQueuePort, SlackThreadReplyInput, SlackThreadReplyPort,
};
use sre_shared::types::{InvestigationJob, InvestigationJobPayload, SlackMessage};
use uuid::Uuid;

use crate::investigation::{InvestigationLogger, string_log_meta};

pub struct EnqueueSlackEventUseCaseDeps {
    pub job_queue: Arc<InvestigationJobQueuePort>,
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

pub struct EnqueueSlackEventUseCase {
    deps: EnqueueSlackEventUseCaseDeps,
}

impl EnqueueSlackEventUseCase {
    pub fn new(deps: EnqueueSlackEventUseCaseDeps) -> Self {
        Self { deps }
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

        match self.deps.job_queue.enqueue(job.clone()).await {
            Ok(()) => {
                let worker_queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
                    job_queue: Arc::clone(&self.deps.job_queue),
                    logger: Arc::clone(&self.deps.logger),
                })
                .await;

                self.deps.logger.info(
                    "Queued investigation job",
                    string_log_meta([
                        ("slackEventId", message.slack_event_id),
                        ("jobId", job.job_id),
                        ("channel", message.channel),
                        ("threadTs", thread_ts),
                        (
                            "ingress_ack_latency_ms",
                            event_started_at.elapsed().as_millis().to_string(),
                        ),
                        ("worker_queue_depth", worker_queue_depth),
                    ]),
                );

                Ok(())
            }
            Err(enqueue_error) => {
                self.deps.logger.error(
                    "Failed to enqueue investigation job",
                    string_log_meta([
                        ("slackEventId", message.slack_event_id),
                        ("jobId", job.job_id),
                        ("channel", message.channel.clone()),
                        ("threadTs", thread_ts.clone()),
                        ("error", enqueue_error.message.clone()),
                        (
                            "ingress_ack_latency_ms",
                            event_started_at.elapsed().as_millis().to_string(),
                        ),
                    ]),
                );

                self.deps
                    .slack_reply_port
                    .post_thread_reply(SlackThreadReplyInput {
                        channel: message.channel,
                        thread_ts,
                        text: format!("Failed to queue investigation: {}", enqueue_error.message),
                    })
                    .await
            }
        }
    }
}

struct ReadWorkerQueueDepthInput {
    job_queue: Arc<InvestigationJobQueuePort>,
    logger: Arc<dyn InvestigationLogger>,
}

async fn read_worker_queue_depth(input: ReadWorkerQueueDepthInput) -> String {
    match input.job_queue.get_depth().await {
        Ok(value) => value.to_string(),
        Err(error) => {
            input.logger.warn(
                "Failed to read worker queue depth after enqueue",
                string_log_meta([("error", error.message)]),
            );
            "unknown".to_string()
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

#[cfg(test)]
mod tests {
    use super::{
        EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, InvestigationLogger, PortError,
        SlackMessage, SlackMessageHandlerPort, SlackThreadReplyInput, SlackThreadReplyPort,
    };
    use crate::investigation::InvestigationLogMeta;
    use async_trait::async_trait;
    use serde_json::Value;
    use sre_shared::ports::outbound::{
        CompleteJobInput, FailJobInput, InvestigationJobQueuePort, JobFailResult, JobQueuePort,
    };
    use sre_shared::types::SlackTriggerType;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use sre_shared::types::InvestigationJob;

    #[derive(Debug, Clone)]
    struct LogEntry {
        message: String,
        meta: InvestigationLogMeta,
    }

    #[derive(Default)]
    struct MockInvestigationJobQueue {
        enqueued_jobs: Mutex<Vec<InvestigationJob>>,
        enqueue_results: Mutex<VecDeque<Result<(), PortError>>>,
        queue_depth_results: Mutex<VecDeque<Result<usize, PortError>>>,
    }

    impl MockInvestigationJobQueue {
        fn enqueued_jobs(&self) -> Vec<InvestigationJob> {
            self.enqueued_jobs
                .lock()
                .expect("lock enqueued jobs")
                .clone()
        }

        fn with_enqueue_results(&self, results: Vec<Result<(), PortError>>) {
            let mut lock = self.enqueue_results.lock().expect("lock enqueue results");
            *lock = VecDeque::from(results);
        }

        fn with_queue_depth_results(&self, results: Vec<Result<usize, PortError>>) {
            let mut lock = self
                .queue_depth_results
                .lock()
                .expect("lock queue depth results");
            *lock = VecDeque::from(results);
        }
    }

    #[async_trait]
    impl JobQueuePort<InvestigationJob> for MockInvestigationJobQueue {
        async fn enqueue(&self, job: InvestigationJob) -> Result<(), PortError> {
            self.enqueued_jobs
                .lock()
                .expect("lock enqueued jobs")
                .push(job);

            let mut lock = self.enqueue_results.lock().expect("lock enqueue results");
            match lock.pop_front() {
                Some(result) => result,
                None => Ok(()),
            }
        }

        async fn claim(&self) -> Result<Option<InvestigationJob>, PortError> {
            Ok(None)
        }

        async fn complete(&self, _input: CompleteJobInput) -> Result<(), PortError> {
            Ok(())
        }

        async fn fail(
            &self,
            _input: FailJobInput,
        ) -> Result<JobFailResult<InvestigationJob>, PortError> {
            Err(PortError::new("fail should not be called in enqueue tests"))
        }

        async fn get_depth(&self) -> Result<usize, PortError> {
            let mut lock = self
                .queue_depth_results
                .lock()
                .expect("lock queue depth results");
            match lock.pop_front() {
                Some(result) => result,
                None => Ok(self.enqueued_jobs().len()),
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
        fn info(&self, message: &str, meta: InvestigationLogMeta) {
            self.infos.lock().expect("lock infos").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }

        fn warn(&self, message: &str, meta: InvestigationLogMeta) {
            self.warns.lock().expect("lock warns").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }

        fn error(&self, message: &str, meta: InvestigationLogMeta) {
            self.errors.lock().expect("lock errors").push(LogEntry {
                message: message.to_string(),
                meta,
            });
        }
    }

    struct TestContext {
        use_case: EnqueueSlackEventUseCase,
        job_queue: Arc<MockInvestigationJobQueue>,
        slack_reply_port: Arc<MockSlackThreadReplyPort>,
        logger: Arc<MockLogger>,
    }

    fn create_use_case(input: CreateUseCaseInput) -> TestContext {
        let job_queue = Arc::new(MockInvestigationJobQueue::default());
        job_queue.with_enqueue_results(input.enqueue_results);
        job_queue.with_queue_depth_results(input.queue_depth_results);
        let slack_reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());

        let use_case = EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::clone(&job_queue) as Arc<InvestigationJobQueuePort>,
            slack_reply_port: Arc::clone(&slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
        });

        TestContext {
            use_case,
            job_queue,
            slack_reply_port,
            logger,
        }
    }

    struct CreateUseCaseInput {
        enqueue_results: Vec<Result<(), PortError>>,
        queue_depth_results: Vec<Result<usize, PortError>>,
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
            enqueue_results: Vec::new(),
            queue_depth_results: Vec::new(),
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let enqueued_jobs = context.job_queue.enqueued_jobs();
        assert_eq!(enqueued_jobs.len(), 1);
        assert_eq!(enqueued_jobs[0].retry_count, 0);
        assert_eq!(context.slack_reply_port.posted_replies().len(), 0);
    }

    #[tokio::test]
    async fn logs_unknown_depth_when_depth_lookup_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_results: Vec::new(),
            queue_depth_results: vec![Err(PortError::new("depth-unavailable"))],
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let warns = context.logger.warns();
        assert_eq!(warns.len(), 1);
        assert_eq!(
            warns[0].message,
            "Failed to read worker queue depth after enqueue"
        );
    }

    #[tokio::test]
    async fn posts_slack_reply_when_enqueue_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_results: vec![Err(PortError::new("fail-1"))],
            queue_depth_results: Vec::new(),
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
            "Failed to queue investigation: fail-1"
        );

        let errors = context.logger.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "Failed to enqueue investigation job");
        assert_eq!(
            errors[0].meta.get("error").and_then(Value::as_str),
            Some("fail-1")
        );
    }
}
