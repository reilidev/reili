use reili_core::task::TaskRequest;

pub fn build_task_prompt(request: &TaskRequest) -> String {
    let trigger_message_text = request.trigger_message.rendered_text();
    let thread_transcript = build_thread_transcript(&request.thread_messages);

    format!("# User\n{trigger_message_text}\n\n# Thread Context\n{thread_transcript}")
}

fn build_thread_transcript(
    messages: &[reili_core::messaging::slack::SlackThreadMessage],
) -> String {
    messages
        .iter()
        .map(|message| {
            let text = message.rendered_text();
            let text = text.trim();
            format!(
                "ts: {}, iso_timestamp: {}, posted_by: {}\nmessage:{}",
                message.ts,
                message.iso_timestamp(),
                message.posted_by(),
                text
            )
        })
        .collect::<Vec<String>>()
        .join("\n---\n")
}

#[cfg(test)]
mod tests {
    use reili_core::messaging::slack::{
        SlackMessage, SlackMessageFile, SlackThreadMessage, SlackTriggerType,
    };
    use reili_core::task::TaskRequest;

    use super::build_task_prompt;

    fn sample_trigger_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "Please investigate this alert".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    #[test]
    fn builds_task_prompt_with_thread_context() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U123".to_string()),
                text: "thread context".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            }],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("# User\nPlease investigate this alert"));
        assert!(prompt.contains("# Thread Context\n"));
        assert!(prompt.contains("posted_by: U123"));
        assert!(prompt.contains("message:thread context"));
    }

    #[test]
    fn formats_task_prompt_with_user_and_thread_context_sections() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![],
        };

        let prompt = build_task_prompt(&request);

        assert_eq!(
            prompt,
            "# User\nPlease investigate this alert\n\n# Thread Context\n"
        );
    }

    #[test]
    fn builds_task_prompt_without_thread_context() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("# User\nPlease investigate this alert"));
        assert!(prompt.contains("# Thread Context\n"));
    }

    #[test]
    fn builds_task_prompt_without_bot_user_you_annotation() {
        let mut trigger = sample_trigger_message();
        trigger.text = "<@U999> investigate this alert".to_string();
        let request = TaskRequest {
            trigger_message: trigger,
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000010".to_string(),
                user: Some("U999".to_string()),
                text: "I started investigation".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            }],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("posted_by: U999"));
        assert!(!prompt.contains("(You)"));
        assert!(prompt.contains("message:I started investigation"));
    }

    #[test]
    fn formats_thread_messages_as_transcript() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U123".to_string()),
                    text: "First message".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: None,
                    text: " follow-up from bot ".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
            ],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains(
            "ts: 1710000000.000001, iso_timestamp: 2024-03-09T16:00:00.000Z, posted_by: U123\nmessage:First message"
        ));
        assert!(prompt.contains(
            "ts: 1710000000.000002, iso_timestamp: 2024-03-09T16:00:00.000Z, posted_by: system\nmessage:follow-up from bot"
        ));
    }

    #[test]
    fn includes_trigger_message_file_plain_text_in_prompt() {
        let mut trigger = sample_trigger_message();
        trigger.text = String::new();
        trigger.files = vec![SlackMessageFile {
            name: Some("aws-health.eml".to_string()),
            title: Some("AWS Health Event".to_string()),
            plain_text: Some("scheduled upgrade required".to_string()),
        }];
        let request = TaskRequest {
            trigger_message: trigger,
            thread_messages: vec![],
        };

        let prompt = build_task_prompt(&request);

        assert!(prompt.contains("# User\nattached_file: aws-health.eml"));
        assert!(prompt.contains("plain_text:\nscheduled upgrade required"));
    }

    #[test]
    fn includes_thread_message_file_plain_text_in_prompt() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000002".to_string(),
                user: Some("U123".to_string()),
                text: String::new(),
                legacy_attachments: Vec::new(),
                files: vec![SlackMessageFile {
                    name: Some("aws-health.eml".to_string()),
                    title: Some("AWS Health Event".to_string()),
                    plain_text: Some("scheduled upgrade required".to_string()),
                }],
                metadata: None,
            }],
        };

        let prompt = build_task_prompt(&request);

        assert!(prompt.contains("posted_by: U123"));
        assert!(prompt.contains("message:attached_file: aws-health.eml"));
        assert!(prompt.contains("plain_text:\nscheduled upgrade required"));
    }
}
