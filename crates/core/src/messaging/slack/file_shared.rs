use async_trait::async_trait;

use crate::error::PortError;

use super::SlackMessage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackFileSharedEvent {
    pub slack_event_id: String,
    pub team_id: String,
    pub channel_id: String,
    pub file_id: String,
    pub user_id: String,
    pub event_ts: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackFileSharedMessagePort: Send + Sync {
    async fn fetch_shared_file_message(
        &self,
        event: SlackFileSharedEvent,
    ) -> Result<Option<SlackMessage>, PortError>;
}
