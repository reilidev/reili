use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackMessageMetadata {
    pub event_type: String,
    pub event_payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackThreadMessage {
    pub ts: String,
    pub user: Option<String>,
    pub text: String,
    pub metadata: Option<SlackMessageMetadata>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SlackMessageMetadata, SlackThreadMessage};

    #[test]
    fn serializes_and_deserializes_thread_message() {
        let value = SlackThreadMessage {
            ts: "123.456".to_string(),
            user: Some("U001".to_string()),
            text: "thread".to_string(),
            metadata: Some(SlackMessageMetadata {
                event_type: "task_control_message_posted".to_string(),
                event_payload: json!({
                    "job_id": "job-1",
                }),
            }),
        };

        let json = serde_json::to_string(&value).expect("serialize thread message");
        let restored: SlackThreadMessage =
            serde_json::from_str(&json).expect("deserialize thread message");

        assert_eq!(restored, value);
    }
}
