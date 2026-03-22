use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

use super::SlackThreadMessage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchSlackThreadHistoryInput {
    pub channel: String,
    pub thread_ts: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackThreadHistoryPort: Send + Sync {
    async fn fetch_thread_history(
        &self,
        input: FetchSlackThreadHistoryInput,
    ) -> Result<Vec<SlackThreadMessage>, PortError>;
}
