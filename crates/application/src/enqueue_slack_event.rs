use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::investigation::{InvestigationJob, InvestigationJobPayload};
use reili_core::messaging::slack::SlackMessage;
use reili_core::messaging::slack::SlackMessageHandlerPort;
use reili_core::messaging::slack::{SlackThreadReplyInput, SlackThreadReplyPort};
use reili_core::queue::InvestigationJobQueuePort;
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
    use reili_core::logger::MockLogger;
    use reili_core::messaging::slack::{MockSlackThreadReplyPort, SlackTriggerType};
    use reili_core::queue::{InvestigationJobQueuePort, MockJobQueuePort};
    use std::sync::{Arc, Mutex};

    use reili_core::investigation::InvestigationJob;

    struct TestContext {
        use_case: EnqueueSlackEventUseCase,
        enqueued_jobs: Arc<Mutex<Vec<InvestigationJob>>>,
        posted_replies: Arc<Mutex<Vec<SlackThreadReplyInput>>>,
    }

    fn create_use_case(input: CreateUseCaseInput) -> TestContext {
        let enqueued_jobs = Arc::new(Mutex::new(Vec::new()));
        let posted_replies = Arc::new(Mutex::new(Vec::new()));
        let mut job_queue = MockJobQueuePort::<InvestigationJob>::new();
        let enqueue_result = input.enqueue_result.clone();
        let should_post_reply = input.enqueue_result.is_err();
        let expected_info_calls = usize::from(input.enqueue_result.is_ok());
        let expected_warn_calls = usize::from(matches!(
            (&input.enqueue_result, &input.queue_depth_result),
            (Ok(()), Some(Err(_)))
        ));
        let expected_error_calls = usize::from(input.enqueue_result.is_err());
        let enqueue_calls = Arc::clone(&enqueued_jobs);
        job_queue
            .expect_enqueue()
            .times(1)
            .returning(move |job: InvestigationJob| {
                enqueue_calls.lock().expect("lock enqueued jobs").push(job);
                enqueue_result.clone()
            });

        match input.queue_depth_result {
            Some(result) => {
                job_queue
                    .expect_get_depth()
                    .times(1)
                    .return_const(result.clone());
            }
            None => {
                job_queue.expect_get_depth().times(0);
            }
        }

        let mut slack_reply_port = MockSlackThreadReplyPort::new();
        if should_post_reply {
            let reply_calls = Arc::clone(&posted_replies);
            slack_reply_port
                .expect_post_thread_reply()
                .times(1)
                .returning(move |input: SlackThreadReplyInput| {
                    reply_calls.lock().expect("lock posted replies").push(input);
                    Ok(())
                });
        } else {
            slack_reply_port.expect_post_thread_reply().times(0);
        }

        let mut logger = MockLogger::new();
        logger
            .expect_info()
            .times(expected_info_calls)
            .return_const(());
        logger
            .expect_warn()
            .times(expected_warn_calls)
            .return_const(());
        logger
            .expect_error()
            .times(expected_error_calls)
            .return_const(());

        let use_case = EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::new(job_queue) as Arc<InvestigationJobQueuePort>,
            slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            logger: Arc::new(logger) as Arc<dyn InvestigationLogger>,
        });

        TestContext {
            use_case,
            enqueued_jobs,
            posted_replies,
        }
    }

    struct CreateUseCaseInput {
        enqueue_result: Result<(), PortError>,
        queue_depth_result: Option<Result<usize, PortError>>,
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
            enqueue_result: Ok(()),
            queue_depth_result: Some(Ok(1)),
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let enqueued_jobs = context
            .enqueued_jobs
            .lock()
            .expect("lock enqueued jobs")
            .clone();
        assert_eq!(enqueued_jobs.len(), 1);
        assert_eq!(enqueued_jobs[0].retry_count, 0);
        assert!(
            context
                .posted_replies
                .lock()
                .expect("lock posted replies")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn continues_when_depth_lookup_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            queue_depth_result: Some(Err(PortError::new("depth-unavailable"))),
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");
    }

    #[tokio::test]
    async fn posts_slack_reply_when_enqueue_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Err(PortError::new("fail-1")),
            queue_depth_result: None,
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");

        let posted_replies = context
            .posted_replies
            .lock()
            .expect("lock posted replies")
            .clone();
        assert_eq!(posted_replies.len(), 1);
        assert_eq!(posted_replies[0].channel, "C001");
        assert_eq!(posted_replies[0].thread_ts, "1710000000.000001");
        assert_eq!(
            posted_replies[0].text,
            "Failed to queue investigation: fail-1"
        );
    }
}
