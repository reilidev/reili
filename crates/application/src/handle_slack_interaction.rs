use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackInteraction, SlackInteractionHandlerPort, SlackTaskControlMessagePort,
};
use reili_core::queue::TaskJobQueuePort;

use crate::cancel_task::{CancelTaskInput, CancelTaskUseCase, CancelTaskUseCaseDeps};
use crate::task::services::InFlightJobRegistry;
use crate::task::{TaskLogger, string_log_meta};

pub struct HandleSlackInteractionUseCaseDeps {
    pub job_queue: Arc<TaskJobQueuePort>,
    pub in_flight_job_registry: InFlightJobRegistry,
    pub slack_task_control_message_port: Arc<dyn SlackTaskControlMessagePort>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct HandleSlackInteractionUseCase {
    cancel_task_use_case: CancelTaskUseCase,
    logger: Arc<dyn TaskLogger>,
}

impl HandleSlackInteractionUseCase {
    pub fn new(deps: HandleSlackInteractionUseCaseDeps) -> Self {
        Self {
            cancel_task_use_case: CancelTaskUseCase::new(CancelTaskUseCaseDeps {
                job_queue: deps.job_queue,
                in_flight_job_registry: deps.in_flight_job_registry,
                slack_task_control_message_port: deps.slack_task_control_message_port,
                logger: Arc::clone(&deps.logger),
            }),
            logger: deps.logger,
        }
    }
}

#[async_trait]
impl SlackInteractionHandlerPort for HandleSlackInteractionUseCase {
    async fn handle(&self, interaction: SlackInteraction) -> Result<(), PortError> {
        match interaction {
            SlackInteraction::CancelJob(input) => {
                self.logger.info(
                    "job_cancel_requested",
                    string_log_meta([
                        ("jobId", input.job_id.clone()),
                        ("requestedBy", input.user_id.clone()),
                        ("channel", input.channel.clone()),
                        ("threadTs", input.thread_ts.clone()),
                        ("messageTs", input.message_ts.clone()),
                    ]),
                );

                self.cancel_task_use_case
                    .execute(CancelTaskInput {
                        job_id: input.job_id,
                        requested_by_user_id: input.user_id,
                        channel: input.channel,
                        thread_ts: input.thread_ts,
                        message_ts: input.message_ts,
                    })
                    .await
            }
        }
    }
}
