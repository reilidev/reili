use async_trait::async_trait;

use crate::error::PortError;

use super::SlackMessage;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait SlackMessageHandlerPort: Send + Sync {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError>;
}
