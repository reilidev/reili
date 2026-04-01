use async_trait::async_trait;

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackTaskControlState {
    Queued,
    Running,
    CancellationRequested { requested_by_user_id: String },
    Cancelled { cancelled_by_user_id: String },
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostTaskControlMessageInput {
    pub channel: String,
    pub thread_ts: String,
    pub job_id: String,
    pub state: SlackTaskControlState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostTaskControlMessageOutput {
    pub message_ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTaskControlMessageInput {
    pub channel: String,
    pub thread_ts: String,
    pub message_ts: String,
    pub job_id: String,
    pub state: SlackTaskControlState,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackTaskControlMessagePort: Send + Sync {
    async fn post_task_control_message(
        &self,
        input: PostTaskControlMessageInput,
    ) -> Result<PostTaskControlMessageOutput, PortError>;

    async fn update_task_control_message(
        &self,
        input: UpdateTaskControlMessageInput,
    ) -> Result<(), PortError>;
}
