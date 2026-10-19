use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackPostMessageInput {
    pub channel: String,
    /// Posts into this thread; `None` posts to the channel itself.
    #[serde(default)]
    pub thread_ts: Option<String>,
    pub text: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackPostMessagePort: Send + Sync {
    async fn post_message(&self, input: SlackPostMessageInput) -> Result<(), PortError>;
}
