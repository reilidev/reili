use serde::{Deserialize, Serialize};

use super::{
    SlackLegacyAttachment, SlackMessageFile, render_slack_legacy_attachments_text,
    render_slack_message_files_text,
};
use crate::secret::SecretString;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_token: Option<SecretString>,
    pub trigger: SlackTriggerType,
    pub channel: String,
    pub user: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_attachments: Vec<SlackLegacyAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SlackMessageFile>,
    pub ts: String,
    pub thread_ts: Option<String>,
}

impl SlackMessage {
    pub fn thread_ts_or_ts(&self) -> &str {
        self.thread_ts.as_deref().unwrap_or(self.ts.as_str())
    }

    pub fn rendered_text(&self) -> String {
        let mut sections = Vec::new();
        if !self.text.trim().is_empty() {
            sections.push(self.text.trim().to_string());
        } else if let Some(attachments_text) =
            render_slack_legacy_attachments_text(&self.legacy_attachments)
        {
            sections.push(attachments_text);
        }

        if let Some(files_text) = render_slack_message_files_text(&self.files) {
            sections.push(files_text);
        }

        sections.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{SlackMessage, SlackTriggerType};
    use crate::messaging::slack::SlackMessageFile;
    use crate::secret::SecretString;

    #[test]
    fn serializes_and_deserializes_slack_message() {
        let value = SlackMessage {
            slack_event_id: "evt-1".to_string(),
            team_id: Some("T001".to_string()),
            action_token: Some(SecretString::from("action-token")),
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "hello".to_string(),
            legacy_attachments: Vec::new(),
            files: vec![SlackMessageFile {
                name: Some("alert.eml".to_string()),
                title: None,
                plain_text: Some("notice".to_string()),
            }],
            ts: "123.456".to_string(),
            thread_ts: None,
        };

        let json = serde_json::to_string(&value).expect("serialize slack message");
        let restored: SlackMessage =
            serde_json::from_str(&json).expect("deserialize slack message");

        assert_eq!(restored, value);
    }

    #[test]
    fn returns_thread_ts_when_present() {
        let message = SlackMessage {
            slack_event_id: "evt-1".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "hello".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "123.456".to_string(),
            thread_ts: Some("123.450".to_string()),
        };

        assert_eq!(message.thread_ts_or_ts(), "123.450");
    }

    #[test]
    fn returns_ts_when_thread_ts_is_absent() {
        let message = SlackMessage {
            slack_event_id: "evt-1".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "hello".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "123.456".to_string(),
            thread_ts: None,
        };

        assert_eq!(message.thread_ts_or_ts(), "123.456");
    }

    #[test]
    fn deserializes_without_action_token_for_backward_compatibility() {
        let json = r#"{"slackEventId":"evt-1","teamId":"T001","trigger":"message","channel":"C001","user":"U001","text":"hello","ts":"123.456","threadTs":null}"#;

        let restored: SlackMessage =
            serde_json::from_str(json).expect("deserialize slack message without action token");

        assert_eq!(restored.action_token, None);
        assert!(restored.legacy_attachments.is_empty());
        assert!(restored.files.is_empty());
    }

    #[test]
    fn renders_text_with_attached_file_plain_text() {
        let message = SlackMessage {
            slack_event_id: "evt-1".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::Message,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "please investigate".to_string(),
            legacy_attachments: Vec::new(),
            files: vec![SlackMessageFile {
                name: Some("alert.eml".to_string()),
                title: None,
                plain_text: Some("scheduled upgrade required".to_string()),
            }],
            ts: "123.456".to_string(),
            thread_ts: None,
        };

        assert_eq!(
            message.rendered_text(),
            "please investigate\n\nattached_file: alert.eml\nplain_text:\nscheduled upgrade required"
        );
    }
}
