use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    SlackLegacyAttachment, SlackMessageFile, render_slack_legacy_attachments_text,
    render_slack_message_files_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessageMetadata {
    pub event_type: String,
    pub event_payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn posted_by(&self) -> &str {
        self.user
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("system")
    }

    pub fn iso_timestamp(&self) -> String {
        let mut parts = self.ts.split('.');
        let seconds_part = parts.next().unwrap_or_default();
        let milliseconds_part = parts.next().unwrap_or("0");

        let seconds = match seconds_part.parse::<i64>() {
            Ok(value) => value,
            Err(_) => return "unknown".to_string(),
        };

        let mut normalized_milliseconds = milliseconds_part.to_string();
        while normalized_milliseconds.len() < 3 {
            normalized_milliseconds.push('0');
        }
        let milliseconds_slice = normalized_milliseconds.chars().take(3).collect::<String>();

        let milliseconds = match milliseconds_slice.parse::<i64>() {
            Ok(value) => value,
            Err(_) => return "unknown".to_string(),
        };

        let unix_millis = match seconds
            .checked_mul(1_000)
            .and_then(|value| value.checked_add(milliseconds))
        {
            Some(value) => value,
            None => return "unknown".to_string(),
        };

        match DateTime::<Utc>::from_timestamp_millis(unix_millis) {
            Some(value) => value.to_rfc3339_opts(SecondsFormat::Millis, true),
            None => "unknown".to_string(),
        }
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

    #[test]
    fn returns_posted_by_user_when_user_is_present() {
        let message = SlackThreadMessage {
            ts: "1710000000.000001".to_string(),
            user: Some(" U123 ".to_string()),
            text: "thread".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            metadata: None,
        };

        assert_eq!(message.posted_by(), "U123");
    }

    #[test]
    fn returns_system_as_posted_by_when_user_is_missing() {
        let message = SlackThreadMessage {
            ts: "1710000000.000001".to_string(),
            user: None,
            text: "thread".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            metadata: None,
        };

        assert_eq!(message.posted_by(), "system");
    }

    #[test]
    fn formats_slack_timestamp_as_iso_timestamp() {
        let message = SlackThreadMessage {
            ts: "1710000000.000001".to_string(),
            user: Some("U001".to_string()),
            text: "thread".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            metadata: None,
        };

        assert_eq!(message.iso_timestamp(), "2024-03-09T16:00:00.000Z");
    }

    #[test]
    fn returns_unknown_iso_timestamp_when_slack_timestamp_is_invalid() {
        let message = SlackThreadMessage {
            ts: "invalid".to_string(),
            user: Some("U001".to_string()),
            text: "thread".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            metadata: None,
        };

        assert_eq!(message.iso_timestamp(), "unknown");
    }
}
