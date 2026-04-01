use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackCancelJobInteraction {
    pub job_id: String,
    pub user_id: String,
    pub channel: String,
    pub thread_ts: String,
    pub message_ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlackInteraction {
    CancelJob(SlackCancelJobInteraction),
}
