use serde::{Deserialize, Serialize};

use crate::messaging::slack::SlackMessage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobPayload {
    pub slack_event_id: String,
    pub message: SlackMessage,
    pub control_message_ts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJob {
    pub job_id: String,
    pub received_at: String,
    pub payload: TaskJobPayload,
    pub retry_count: u32,
}

#[cfg(test)]
mod tests {
    use super::{TaskJob, TaskJobPayload};
    use crate::messaging::slack::{SlackMessage, SlackTriggerType};

    #[test]
    fn serializes_and_deserializes_task_job() {
        let value = TaskJob {
            job_id: "job-1".to_string(),
            received_at: "2026-03-04T00:00:00Z".to_string(),
            payload: TaskJobPayload {
                slack_event_id: "evt-1".to_string(),
                message: SlackMessage {
                    slack_event_id: "evt-1".to_string(),
                    team_id: Some("T001".to_string()),
                    action_token: None,
                    trigger: SlackTriggerType::AppMention,
                    channel: "C001".to_string(),
                    user: "U001".to_string(),
                    text: "check alert".to_string(),
                    ts: "123.456".to_string(),
                    thread_ts: Some("123.450".to_string()),
                },
                control_message_ts: "123.457".to_string(),
            },
            retry_count: 0,
        };

        let json = serde_json::to_string(&value).expect("serialize task job");
        let restored: TaskJob = serde_json::from_str(&json).expect("deserialize task job");

        assert_eq!(restored, value);
    }
}
