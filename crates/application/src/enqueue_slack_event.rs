use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::SlackMessage;
use reili_core::messaging::slack::SlackMessageHandlerPort;
use reili_core::messaging::slack::{
    AddSlackReactionInput, SlackReactionPort, SlackThreadReplyInput, SlackThreadReplyPort,
};
use reili_core::queue::TaskJobQueuePort;
use reili_core::task::{TaskJob, TaskJobPayload};
use uuid::Uuid;

use crate::task::{TaskLogger, string_log_meta};

const QUEUED_REACTION_NAME: &str = "eyes";
const SLACK_ALREADY_REACTED_ERROR_CODE: &str = "already_reacted";

pub struct EnqueueSlackEventUseCaseDeps {
    pub job_queue: Arc<TaskJobQueuePort>,
    pub slack_reaction_port: Arc<dyn SlackReactionPort>,
    pub slack_reply_port: Arc<dyn SlackThreadReplyPort>,
    pub logger: Arc<dyn TaskLogger>,
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
        let job = build_task_job(BuildTaskJobInput {
            message: message.clone(),
            received_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        });

        match self.deps.job_queue.enqueue(job.clone()).await {
            Ok(()) => {
                add_queue_accepted_reaction(AddQueueAcceptedReactionInput {
                    slack_reaction_port: Arc::clone(&self.deps.slack_reaction_port),
                    logger: Arc::clone(&self.deps.logger),
                    slack_event_id: message.slack_event_id.clone(),
                    reaction: AddSlackReactionInput {
                        channel: message.channel.clone(),
                        message_ts: message.ts.clone(),
                        name: QUEUED_REACTION_NAME.to_string(),
                    },
                })
                .await;

                let worker_queue_depth = read_worker_queue_depth(ReadWorkerQueueDepthInput {
                    job_queue: Arc::clone(&self.deps.job_queue),
                    logger: Arc::clone(&self.deps.logger),
                })
                .await;

                self.deps.logger.info(
                    "Queued task job",
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
                    "Failed to enqueue task job",
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
                        text: format!("Failed to queue task: {}", enqueue_error.message),
                    })
                    .await
            }
        }
    }
}

struct AddQueueAcceptedReactionInput {
    slack_reaction_port: Arc<dyn SlackReactionPort>,
    logger: Arc<dyn TaskLogger>,
    slack_event_id: String,
    reaction: AddSlackReactionInput,
}

async fn add_queue_accepted_reaction(input: AddQueueAcceptedReactionInput) {
    let AddQueueAcceptedReactionInput {
        slack_reaction_port,
        logger,
        slack_event_id,
        reaction,
    } = input;
    let channel = reaction.channel.clone();
    let message_ts = reaction.message_ts.clone();
    let emoji = reaction.name.clone();

    match slack_reaction_port.add_reaction(reaction).await {
        Ok(()) => {}
        Err(error) if error.is_service_error_code(SLACK_ALREADY_REACTED_ERROR_CODE) => {}
        Err(error) => {
            logger.warn(
                "Failed to add Slack reaction after enqueue",
                string_log_meta([
                    ("slackEventId", slack_event_id),
                    ("channel", channel),
                    ("messageTs", message_ts),
                    ("emoji", emoji),
                    ("error", error.message),
                ]),
            );
        }
    }
}

struct ReadWorkerQueueDepthInput {
    job_queue: Arc<TaskJobQueuePort>,
    logger: Arc<dyn TaskLogger>,
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

struct BuildTaskJobInput {
    message: SlackMessage,
    received_at: String,
}

fn build_task_job(input: BuildTaskJobInput) -> TaskJob {
    let slack_event_id = input.message.slack_event_id.clone();

    TaskJob {
        job_id: Uuid::new_v4().to_string(),
        received_at: input.received_at,
        payload: TaskJobPayload {
            slack_event_id,
            message: input.message,
        },
        retry_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EnqueueSlackEventUseCase, EnqueueSlackEventUseCaseDeps, PortError, SlackMessage,
        SlackMessageHandlerPort, SlackThreadReplyInput, SlackThreadReplyPort, TaskLogger,
    };
    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        AddSlackReactionInput, MockSlackReactionPort, MockSlackThreadReplyPort, SlackReactionPort,
        SlackTriggerType,
    };
    use reili_core::queue::{MockJobQueuePort, TaskJobQueuePort};
    use std::sync::{Arc, Mutex};

    use reili_core::task::TaskJob;

    #[derive(Default)]
    struct NoopLogger;

    impl TaskLogger for NoopLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    struct TestContext {
        use_case: EnqueueSlackEventUseCase,
        enqueued_jobs: Arc<Mutex<Vec<TaskJob>>>,
        added_reactions: Arc<Mutex<Vec<AddSlackReactionInput>>>,
        posted_replies: Arc<Mutex<Vec<SlackThreadReplyInput>>>,
    }

    fn create_use_case(input: CreateUseCaseInput) -> TestContext {
        let enqueued_jobs = Arc::new(Mutex::new(Vec::new()));
        let added_reactions = Arc::new(Mutex::new(Vec::new()));
        let posted_replies = Arc::new(Mutex::new(Vec::new()));
        let mut job_queue = MockJobQueuePort::<TaskJob>::new();
        let enqueue_result = input.enqueue_result.clone();
        let should_post_reply = input.enqueue_result.is_err();
        let enqueue_calls = Arc::clone(&enqueued_jobs);
        job_queue
            .expect_enqueue()
            .times(1)
            .returning(move |job: TaskJob| {
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

        let mut slack_reaction_port = MockSlackReactionPort::new();
        if input.enqueue_result.is_ok() {
            let reaction_result = input.reaction_result.clone();
            let reaction_calls = Arc::clone(&added_reactions);
            slack_reaction_port
                .expect_add_reaction()
                .times(1)
                .returning(move |input: AddSlackReactionInput| {
                    reaction_calls
                        .lock()
                        .expect("lock added reactions")
                        .push(input);
                    reaction_result.clone()
                });
        } else {
            slack_reaction_port.expect_add_reaction().times(0);
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

        let logger = Arc::new(NoopLogger);

        let use_case = EnqueueSlackEventUseCase::new(EnqueueSlackEventUseCaseDeps {
            job_queue: Arc::new(job_queue) as Arc<TaskJobQueuePort>,
            slack_reaction_port: Arc::new(slack_reaction_port) as Arc<dyn SlackReactionPort>,
            slack_reply_port: Arc::new(slack_reply_port) as Arc<dyn SlackThreadReplyPort>,
            logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
        });

        TestContext {
            use_case,
            enqueued_jobs,
            added_reactions,
            posted_replies,
        }
    }

    struct CreateUseCaseInput {
        enqueue_result: Result<(), PortError>,
        reaction_result: Result<(), PortError>,
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
    async fn dispatches_alert_task_job() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            reaction_result: Ok(()),
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
        assert_eq!(
            context
                .added_reactions
                .lock()
                .expect("lock added reactions")
                .clone(),
            vec![AddSlackReactionInput {
                channel: "C001".to_string(),
                message_ts: "1710000000.000001".to_string(),
                name: "eyes".to_string(),
            }]
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
    async fn adds_reaction_to_reply_message_ts_instead_of_thread_root() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            reaction_result: Ok(()),
            queue_depth_result: Some(Ok(1)),
        });

        let mut message = create_message();
        message.ts = "1710000000.000010".to_string();
        message.thread_ts = Some("1710000000.000001".to_string());

        context
            .use_case
            .handle(message)
            .await
            .expect("enqueue handle");

        let added_reactions = context
            .added_reactions
            .lock()
            .expect("lock added reactions")
            .clone();
        assert_eq!(added_reactions.len(), 1);
        assert_eq!(added_reactions[0].message_ts, "1710000000.000010");
    }

    #[tokio::test]
    async fn continues_when_depth_lookup_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            reaction_result: Ok(()),
            queue_depth_result: Some(Err(PortError::new("depth-unavailable"))),
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");
    }

    #[tokio::test]
    async fn continues_when_reaction_was_already_added() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            reaction_result: Err(PortError::service_error(
                "already_reacted",
                "Slack API returned error: method=reactions.add error=already_reacted",
            )),
            queue_depth_result: Some(Ok(1)),
        });

        context
            .use_case
            .handle(create_message())
            .await
            .expect("enqueue handle");
    }

    #[tokio::test]
    async fn continues_when_reaction_add_fails() {
        let context = create_use_case(CreateUseCaseInput {
            enqueue_result: Ok(()),
            reaction_result: Err(PortError::new("slack api failed")),
            queue_depth_result: Some(Ok(1)),
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
            reaction_result: Ok(()),
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
        assert_eq!(posted_replies[0].text, "Failed to queue task: fail-1");
        assert!(
            context
                .added_reactions
                .lock()
                .expect("lock added reactions")
                .is_empty()
        );
    }
}
