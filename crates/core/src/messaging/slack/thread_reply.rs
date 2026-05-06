use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackThreadReplyInput {
    pub channel: String,
    pub thread_ts: String,
    pub text: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackThreadReplyPort: Send + Sync {
    async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError>;
}
