use async_trait::async_trait;

use crate::{errors::PortError, types::SlackMessage};

#[async_trait]
pub trait SlackMessageHandlerPort: Send + Sync {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError>;
}
