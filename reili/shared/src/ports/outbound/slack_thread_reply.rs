use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackThreadReplyInput {
    pub channel: String,
    pub thread_ts: String,
    pub text: String,
}

#[async_trait]
pub trait SlackThreadReplyPort: Send + Sync {
    async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError>;
}
