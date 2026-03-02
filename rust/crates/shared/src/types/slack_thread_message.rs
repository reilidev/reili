use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackThreadMessage {
    pub ts: String,
    pub user: Option<String>,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::SlackThreadMessage;

    #[test]
    fn serializes_and_deserializes_thread_message() {
        let value = SlackThreadMessage {
            ts: "123.456".to_string(),
            user: Some("U001".to_string()),
            text: "thread".to_string(),
        };

        let json = serde_json::to_string(&value).expect("serialize thread message");
        let restored: SlackThreadMessage =
            serde_json::from_str(&json).expect("deserialize thread message");

        assert_eq!(restored, value);
    }
}
