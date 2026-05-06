use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSlackReactionInput {
    pub channel: String,
    pub message_ts: String,
    pub name: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackReactionPort: Send + Sync {
    async fn add_reaction(&self, input: AddSlackReactionInput) -> Result<(), PortError>;
}
