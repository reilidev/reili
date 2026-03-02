use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{errors::PortError, types::SlackThreadMessage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchSlackThreadHistoryInput {
    pub channel: String,
    pub thread_ts: String,
}

#[async_trait]
pub trait SlackThreadHistoryPort: Send + Sync {
    async fn fetch_thread_history(
        &self,
        input: FetchSlackThreadHistoryInput,
    ) -> Result<Vec<SlackThreadMessage>, PortError>;
}
