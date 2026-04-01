use async_trait::async_trait;

use crate::error::PortError;

use super::SlackInteraction;

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackInteractionHandlerPort: Send + Sync {
    async fn handle(&self, interaction: SlackInteraction) -> Result<(), PortError>;
}
