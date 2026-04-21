use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackLegacyAttachment, SlackMessage, SlackMessageFile, SlackTriggerType,
    render_slack_legacy_attachments_text, render_slack_message_files_text,
};
use reili_core::secret::SecretString;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "Keep ParsedSlackEvent ergonomic at the runtime boundary without boxing SlackMessage"
)]
pub enum ParsedSlackEvent {
    UrlVerification { challenge: String },
    Message(SlackMessage),
    Ignored,
}

pub fn parse_slack_event(payload: &[u8], bot_user_id: &str) -> Result<ParsedSlackEvent, PortError> {
    let envelope: SlackEventEnvelope = serde_json::from_slice(payload).map_err(|error| {
        PortError::new(format!("Failed to parse Slack event payload JSON: {error}"))
    })?;

    if envelope.envelope_type == "url_verification" {
        let challenge = envelope
            .challenge
            .ok_or_else(|| PortError::new("Slack url_verification payload missing challenge"))?;
        return Ok(ParsedSlackEvent::UrlVerification { challenge });
    }

    if envelope.envelope_type != "event_callback" {
        return Ok(ParsedSlackEvent::Ignored);
    }

    let event_id = match envelope.event_id {
        Some(value) => value,
        None => return Ok(ParsedSlackEvent::Ignored),
    };
    let event = match envelope.event {
        Some(value) => value,
        None => return Ok(ParsedSlackEvent::Ignored),
    };

    match event.event_type.as_str() {
        "message" => parse_message_event(
            ParseEventInput {
                event_id,
                team_id: envelope.team_id,
                event,
                bot_user_id,
            },
            SlackTriggerType::Message,
        ),
        "app_mention" => parse_message_event(
            ParseEventInput {
                event_id,
                team_id: envelope.team_id,
                event,
                bot_user_id,
            },
            SlackTriggerType::AppMention,
        ),
        _ => Ok(ParsedSlackEvent::Ignored),
    }
}

struct ParseEventInput<'a> {
    event_id: String,
    team_id: Option<String>,
    event: SlackCallbackEvent,
    bot_user_id: &'a str,
}

fn parse_message_event(
    input: ParseEventInput<'_>,
    trigger: SlackTriggerType,
) -> Result<ParsedSlackEvent, PortError> {
    if trigger == SlackTriggerType::Message && input.event.subtype.is_some() {
        return Ok(ParsedSlackEvent::Ignored);
    }

    let actor_is_bot = input.event.bot_id.is_some() || input.event.bot_profile.is_some();
    let user = match input.event.user {
        Some(value) => value,
        None => return Ok(ParsedSlackEvent::Ignored),
    };
    if user == input.bot_user_id {
        return Ok(ParsedSlackEvent::Ignored);
    }

    let channel = match input.event.channel {
        Some(value) => value,
        None => return Ok(ParsedSlackEvent::Ignored),
    };
    let legacy_attachments = input.event.attachments.unwrap_or_default();
    let files = input.event.files.unwrap_or_default();
    let text = resolve_message_text(input.event.text, &legacy_attachments).unwrap_or_default();
    if text.trim().is_empty() && render_slack_message_files_text(&files).is_none() {
        return Ok(ParsedSlackEvent::Ignored);
    }
    let ts = match input.event.ts {
        Some(value) => value,
        None => return Ok(ParsedSlackEvent::Ignored),
    };

    let message = SlackMessage {
        slack_event_id: input.event_id,
        team_id: input.team_id,
        action_token: input
            .event
            .assistant_thread
            .and_then(|thread| thread.action_token),
        trigger,
        channel,
        user,
        actor_is_bot,
        text,
        legacy_attachments,
        files,
        ts,
        thread_ts: input.event.thread_ts,
    };

    Ok(ParsedSlackEvent::Message(message))
}

#[derive(Debug, Deserialize)]
struct SlackEventEnvelope {
    #[serde(rename = "type")]
    envelope_type: String,
    challenge: Option<String>,
    event_id: Option<String>,
    team_id: Option<String>,
    event: Option<SlackCallbackEvent>,
}

#[derive(Debug, Deserialize)]
struct SlackCallbackEvent {
    #[serde(rename = "type")]
    event_type: String,
    subtype: Option<String>,
    channel: Option<String>,
    user: Option<String>,
    bot_id: Option<String>,
    bot_profile: Option<serde_json::Value>,
    text: Option<String>,
    attachments: Option<Vec<SlackLegacyAttachment>>,
    files: Option<Vec<SlackMessageFile>>,
    ts: Option<String>,
    thread_ts: Option<String>,
    assistant_thread: Option<SlackAssistantThread>,
}

#[derive(Debug, Deserialize)]
struct SlackAssistantThread {
    action_token: Option<SecretString>,
}

fn resolve_message_text(
    text: Option<String>,
    legacy_attachments: &[SlackLegacyAttachment],
) -> Option<String> {
    match text {
        Some(value) if !value.trim().is_empty() => Some(value),
        _ => render_slack_legacy_attachments_text(legacy_attachments),
    }
}

#[cfg(test)]
mod tests {
    use reili_core::messaging::slack::SlackMessageFile;
    use reili_core::messaging::slack::{SlackLegacyAttachment, SlackMessage, SlackTriggerType};
    use reili_core::secret::SecretString;
    use serde_json::json;

    use super::{ParsedSlackEvent, parse_slack_event};

    #[test]
    fn parses_url_verification_challenge() {
        let parsed = parse_slack_event(
            json!({
                "type": "url_verification",
                "challenge": "challenge-token"
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::UrlVerification {
                challenge: "challenge-token".to_string(),
            }
        );
    }

    #[test]
    fn parses_processable_message_event() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-1",
                "team_id": "T001",
                "event": {
                    "type": "message",
                    "channel": "C001",
                    "user": "U001",
                    "text": "please investigate",
                    "ts": "1710000000.000001",
                    "thread_ts": "1710000000.000000"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-1".to_string(),
                team_id: Some("T001".to_string()),
                action_token: None,
                trigger: SlackTriggerType::Message,
                channel: "C001".to_string(),
                user: "U001".to_string(),
                actor_is_bot: false,
                text: "please investigate".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                ts: "1710000000.000001".to_string(),
                thread_ts: Some("1710000000.000000".to_string()),
            })
        );
    }

    #[test]
    fn parses_processable_app_mention_event() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-2",
                "event": {
                    "type": "app_mention",
                    "assistant_thread": {
                        "action_token": "action-token"
                    },
                    "channel": "C001",
                    "user": "U002",
                    "text": "<@U-BOT> investigate this alert",
                    "ts": "1710000000.000002"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-2".to_string(),
                team_id: None,
                action_token: Some(SecretString::from("action-token")),
                trigger: SlackTriggerType::AppMention,
                channel: "C001".to_string(),
                user: "U002".to_string(),
                actor_is_bot: false,
                text: "<@U-BOT> investigate this alert".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                ts: "1710000000.000002".to_string(),
                thread_ts: None,
            })
        );
    }

    #[test]
    fn reads_action_token_from_assistant_thread() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-3",
                "event": {
                    "type": "app_mention",
                    "assistant_thread": {
                        "action_token": "assistant-thread-token"
                    },
                    "channel": "C001",
                    "user": "U003",
                    "text": "<@U-BOT> search slack",
                    "ts": "1710000000.000003"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-3".to_string(),
                team_id: None,
                action_token: Some(SecretString::from("assistant-thread-token")),
                trigger: SlackTriggerType::AppMention,
                channel: "C001".to_string(),
                user: "U003".to_string(),
                actor_is_bot: false,
                text: "<@U-BOT> search slack".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                ts: "1710000000.000003".to_string(),
                thread_ts: None,
            })
        );
    }

    #[test]
    fn falls_back_to_legacy_attachment_text_when_event_text_is_empty() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-attachment",
                "event": {
                    "type": "message",
                    "channel": "C001",
                    "user": "U004",
                    "text": "",
                    "attachments": [
                        {
                            "pretext": "Alert",
                            "text": "CPU usage is high",
                            "fields": [
                                {
                                    "title": "Service",
                                    "value": "api"
                                }
                            ]
                        }
                    ],
                    "ts": "1710000000.000005"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse attachment event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-attachment".to_string(),
                team_id: None,
                action_token: None,
                trigger: SlackTriggerType::Message,
                channel: "C001".to_string(),
                user: "U004".to_string(),
                actor_is_bot: false,
                text: "<None|Alert|> CPU usage is high".to_string(),
                legacy_attachments: vec![SlackLegacyAttachment {
                    pretext: Some("Alert".to_string()),
                    text: Some("CPU usage is high".to_string()),
                    fields: vec![reili_core::messaging::slack::SlackLegacyAttachmentField {
                        title: Some("Service".to_string()),
                        value: Some("api".to_string()),
                        short: None,
                    }],
                    ..Default::default()
                }],
                files: Vec::new(),
                ts: "1710000000.000005".to_string(),
                thread_ts: None,
            })
        );
    }

    #[test]
    fn parses_file_only_message_event() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-file",
                "event": {
                    "type": "message",
                    "channel": "C001",
                    "user": "U005",
                    "text": "",
                    "files": [{
                        "name": "aws-health.eml",
                        "title": "AWS Health Event",
                        "plain_text": "scheduled upgrade required"
                    }],
                    "ts": "1710000000.000006"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse file event");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-file".to_string(),
                team_id: None,
                action_token: None,
                trigger: SlackTriggerType::Message,
                channel: "C001".to_string(),
                user: "U005".to_string(),
                actor_is_bot: false,
                text: String::new(),
                legacy_attachments: Vec::new(),
                files: vec![SlackMessageFile {
                    name: Some("aws-health.eml".to_string()),
                    title: Some("AWS Health Event".to_string()),
                    plain_text: Some("scheduled upgrade required".to_string()),
                }],
                ts: "1710000000.000006".to_string(),
                thread_ts: None,
            })
        );

        let ParsedSlackEvent::Message(message) = parsed else {
            panic!("expected message event");
        };
        assert_eq!(
            message.rendered_text(),
            "attached_file: aws-health.eml\nplain_text:\nscheduled upgrade required"
        );
    }

    #[test]
    fn ignores_message_events_with_subtype_or_from_bot() {
        let subtype_event = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-subtype",
                "event": {
                    "type": "message",
                    "subtype": "bot_message",
                    "channel": "C001",
                    "user": "U001",
                    "text": "bot",
                    "ts": "1710000000.000003"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse subtype event");
        assert_eq!(subtype_event, ParsedSlackEvent::Ignored);

        let bot_message_event = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-bot",
                "event": {
                    "type": "message",
                    "channel": "C001",
                    "user": "U-BOT",
                    "text": "self message",
                    "ts": "1710000000.000004"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse bot event");
        assert_eq!(bot_message_event, ParsedSlackEvent::Ignored);
    }

    #[test]
    fn marks_app_mention_actor_as_bot_when_bot_id_is_present() {
        let parsed = parse_slack_event(
            json!({
                "type": "event_callback",
                "event_id": "evt-bot-mention",
                "event": {
                    "type": "app_mention",
                    "channel": "C001",
                    "user": "U-BOT-ACTOR",
                    "bot_id": "B001",
                    "text": "<@U-BOT> investigate this alert",
                    "ts": "1710000000.000007"
                }
            })
            .to_string()
            .as_bytes(),
            "U-BOT",
        )
        .expect("parse bot app mention");

        assert_eq!(
            parsed,
            ParsedSlackEvent::Message(SlackMessage {
                slack_event_id: "evt-bot-mention".to_string(),
                team_id: None,
                action_token: None,
                trigger: SlackTriggerType::AppMention,
                channel: "C001".to_string(),
                user: "U-BOT-ACTOR".to_string(),
                actor_is_bot: true,
                text: "<@U-BOT> investigate this alert".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                ts: "1710000000.000007".to_string(),
                thread_ts: None,
            })
        );
    }

    #[test]
    fn returns_error_when_payload_is_invalid_json() {
        let error = parse_slack_event(br#"{invalid json}"#, "U-BOT")
            .expect_err("invalid payload should fail");
        assert!(
            error
                .message
                .contains("Failed to parse Slack event payload JSON")
        );
    }
}
