use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertContext {
    pub raw_text: String,
    pub trigger_message_text: String,
    pub thread_transcript: String,
}

#[cfg(test)]
mod tests {
    use super::AlertContext;

    #[test]
    fn serializes_and_deserializes_alert_context() {
        let value = AlertContext {
            raw_text: "raw".to_string(),
            trigger_message_text: "trigger".to_string(),
            thread_transcript: "thread".to_string(),
        };

        let json = serde_json::to_string(&value).expect("serialize alert context");
        let restored: AlertContext =
            serde_json::from_str(&json).expect("deserialize alert context");

        assert_eq!(restored, value);
    }
}
