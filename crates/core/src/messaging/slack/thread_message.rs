use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    SlackLegacyAttachment, SlackMessageFile, render_slack_legacy_attachments_text,
    render_slack_message_files_text,
};

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_attachments: Vec<SlackLegacyAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SlackMessageFile>,
    pub metadata: Option<SlackMessageMetadata>,
}

impl SlackThreadMessage {
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
    use serde_json::json;

    use super::{SlackMessageMetadata, SlackThreadMessage};
    use crate::messaging::slack::SlackMessageFile;

    #[test]
    fn serializes_and_deserializes_thread_message() {
        let value = SlackThreadMessage {
            ts: "123.456".to_string(),
            user: Some("U001".to_string()),
            text: "thread".to_string(),
            legacy_attachments: Vec::new(),
            files: vec![SlackMessageFile {
                name: Some("thread.txt".to_string()),
                title: None,
                plain_text: Some("details".to_string()),
            }],
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

    #[test]
    fn deserializes_without_legacy_attachments_for_backward_compatibility() {
        let json = r#"{"ts":"123.456","user":"U001","text":"thread","metadata":null}"#;

        let restored: SlackThreadMessage =
            serde_json::from_str(json).expect("deserialize thread message without attachments");

        assert!(restored.legacy_attachments.is_empty());
        assert!(restored.files.is_empty());
    }

    #[test]
    fn renders_thread_text_with_attached_file_plain_text() {
        let message = SlackThreadMessage {
            ts: "123.456".to_string(),
            user: Some("U001".to_string()),
            text: String::new(),
            legacy_attachments: Vec::new(),
            files: vec![SlackMessageFile {
                name: Some("thread.txt".to_string()),
                title: None,
                plain_text: Some("attachment context".to_string()),
            }],
            metadata: None,
        };

        assert_eq!(
            message.rendered_text(),
            "attached_file: thread.txt\nplain_text:\nattachment context"
        );
    }
}
