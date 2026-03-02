use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackTriggerType {
    Message,
    AppMention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackMessage {
    pub slack_event_id: String,
    pub team_id: Option<String>,
    pub trigger: SlackTriggerType,
    pub channel: String,
    pub user: String,
    pub text: String,
    pub ts: String,
    pub thread_ts: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{SlackMessage, SlackTriggerType};

    #[test]
    fn serializes_and_deserializes_slack_message() {
        let value = SlackMessage {
            slack_event_id: "evt-1".to_string(),
            team_id: Some("T001".to_string()),
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "hello".to_string(),
            ts: "123.456".to_string(),
            thread_ts: None,
        };

        let json = serde_json::to_string(&value).expect("serialize slack message");
        let restored: SlackMessage =
            serde_json::from_str(&json).expect("deserialize slack message");

        assert_eq!(restored, value);
    }
}
