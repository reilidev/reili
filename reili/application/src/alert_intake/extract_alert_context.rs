use chrono::{DateTime, SecondsFormat, Utc};
use reili_shared::types::{AlertContext, SlackThreadMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractAlertContextInput {
    pub trigger_message_text: String,
    pub thread_messages: Vec<SlackThreadMessage>,
    pub bot_user_id: Option<String>,
}

#[must_use]
pub fn extract_alert_context(input: ExtractAlertContextInput) -> AlertContext {
    let trigger_message_text = input.trigger_message_text.trim().to_string();
    AlertContext {
        raw_text: trigger_message_text.clone(),
        trigger_message_text,
        thread_transcript: build_thread_transcript(
            &input.thread_messages,
            input.bot_user_id.as_deref(),
        ),
    }
}

fn build_thread_transcript(messages: &[SlackThreadMessage], bot_user_id: Option<&str>) -> String {
    messages
        .iter()
        .map(|message| {
            let author = normalize_author(message.user.as_deref(), bot_user_id);
            let text = message.text.trim();
            let iso_timestamp = to_iso_timestamp(&message.ts);
            format!(
                "[ts: {} | iso: {}] {}: {}",
                message.ts, iso_timestamp, author, text
            )
        })
        .collect::<Vec<String>>()
        .join("\n---\n")
}

fn normalize_author(user: Option<&str>, bot_user_id: Option<&str>) -> String {
    let normalized = match user.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value,
        None => return "system".to_string(),
    };

    if bot_user_id.is_some_and(|bot_user_id_value| normalized == bot_user_id_value) {
        return format!("{normalized} (You)");
    }

    normalized.to_string()
}

fn to_iso_timestamp(ts: &str) -> String {
    let mut parts = ts.split('.');
    let seconds_part = parts.next().unwrap_or_default();
    let milliseconds_part = parts.next().unwrap_or("0");

    let seconds = match seconds_part.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return "unknown".to_string(),
    };

    let milliseconds_slice = normalize_milliseconds(milliseconds_part);
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

fn normalize_milliseconds(milliseconds_part: &str) -> String {
    let mut normalized = milliseconds_part.to_string();
    while normalized.len() < 3 {
        normalized.push('0');
    }

    normalized.chars().take(3).collect()
}

#[cfg(test)]
mod tests {
    use reili_shared::types::SlackThreadMessage;

    use super::{ExtractAlertContextInput, extract_alert_context};

    #[test]
    fn returns_trigger_message_text_and_empty_thread_transcript() {
        let result = extract_alert_context(ExtractAlertContextInput {
            trigger_message_text: "monitor alert".to_string(),
            thread_messages: Vec::new(),
            bot_user_id: None,
        });

        assert_eq!(result.raw_text, "monitor alert");
        assert_eq!(result.trigger_message_text, "monitor alert");
        assert_eq!(result.thread_transcript, "");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let result = extract_alert_context(ExtractAlertContextInput {
            trigger_message_text: "  datadog monitor  ".to_string(),
            thread_messages: Vec::new(),
            bot_user_id: None,
        });

        assert_eq!(result.raw_text, "datadog monitor");
        assert_eq!(result.trigger_message_text, "datadog monitor");
        assert_eq!(result.thread_transcript, "");
    }

    #[test]
    fn formats_thread_messages_as_transcript() {
        let result = extract_alert_context(ExtractAlertContextInput {
            trigger_message_text: "alert".to_string(),
            thread_messages: vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U123".to_string()),
                    text: "First message".to_string(),
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: None,
                    text: " follow-up from bot ".to_string(),
                },
            ],
            bot_user_id: None,
        });

        assert_eq!(result.raw_text, "alert");
        assert_eq!(result.trigger_message_text, "alert");
        assert_eq!(
            result.thread_transcript,
            "[ts: 1710000000.000001 | iso: 2024-03-09T16:00:00.000Z] U123: First message\n---\n[ts: 1710000000.000002 | iso: 2024-03-09T16:00:00.000Z] system: follow-up from bot"
        );
    }

    #[test]
    fn appends_you_when_author_matches_bot_user_id() {
        let result = extract_alert_context(ExtractAlertContextInput {
            trigger_message_text: "<@U999> investigate this alert".to_string(),
            bot_user_id: Some("U999".to_string()),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000010".to_string(),
                user: Some("U999".to_string()),
                text: "I started investigation".to_string(),
            }],
        });

        assert_eq!(result.raw_text, "<@U999> investigate this alert");
        assert_eq!(
            result.trigger_message_text,
            "<@U999> investigate this alert"
        );
        assert_eq!(
            result.thread_transcript,
            "[ts: 1710000000.000010 | iso: 2024-03-09T16:00:00.000Z] U999 (You): I started investigation"
        );
    }
}
