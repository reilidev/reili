use reili_core::error::PortError;
use reili_core::messaging::slack::{SlackCancelJobInteraction, SlackInteraction};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub struct SlackInteractionForm {
    #[serde(deserialize_with = "deserialize_slack_interaction")]
    pub payload: ParsedSlackInteraction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSlackInteraction {
    Interaction(SlackInteraction),
    Ignored,
}

pub fn parse_slack_interaction_value(
    value: serde_json::Value,
) -> Result<ParsedSlackInteraction, PortError> {
    let payload: SlackInteractionPayload = serde_json::from_value(value).map_err(|error| {
        PortError::new(format!(
            "Failed to parse Slack interaction payload JSON: {error}"
        ))
    })?;

    Ok(match payload {
        SlackInteractionPayload::BlockActions(payload) => payload.into(),
        SlackInteractionPayload::Other => ParsedSlackInteraction::Ignored,
    })
}

fn deserialize_slack_interaction<'de, D>(
    deserializer: D,
) -> Result<ParsedSlackInteraction, D::Error>
where
    D: Deserializer<'de>,
{
    let payload = String::deserialize(deserializer)?;
    let payload: SlackInteractionPayload = serde_json::from_str(&payload).map_err(|error| {
        D::Error::custom(format!(
            "Failed to parse Slack interaction payload JSON: {error}"
        ))
    })?;

    Ok(match payload {
        SlackInteractionPayload::BlockActions(payload) => payload.into(),
        SlackInteractionPayload::Other => ParsedSlackInteraction::Ignored,
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SlackInteractionPayload {
    BlockActions(SlackBlockActionsPayload),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SlackBlockActionsPayload {
    user: SlackInteractionUser,
    channel: SlackInteractionChannel,
    message: SlackInteractionMessage,
    #[serde(
        rename = "actions",
        deserialize_with = "deserialize_cancel_task_action"
    )]
    cancel_action: Option<SlackCancelTaskAction>,
}

impl From<SlackBlockActionsPayload> for ParsedSlackInteraction {
    fn from(payload: SlackBlockActionsPayload) -> Self {
        let Some(action) = payload.cancel_action else {
            return ParsedSlackInteraction::Ignored;
        };

        ParsedSlackInteraction::Interaction(SlackInteraction::CancelJob(
            SlackCancelJobInteraction {
                job_id: action.value,
                user_id: payload.user.id,
                channel: payload.channel.id,
                thread_ts: payload
                    .message
                    .thread_ts
                    .unwrap_or_else(|| payload.message.ts.clone()),
                message_ts: payload.message.ts,
            },
        ))
    }
}

#[derive(Debug, Deserialize)]
struct SlackInteractionUser {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SlackInteractionChannel {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SlackInteractionMessage {
    ts: String,
    thread_ts: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action_id", rename_all = "snake_case")]
enum SlackInteractionAction {
    CancelTask(SlackCancelTaskAction),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SlackCancelTaskAction {
    value: String,
}

fn deserialize_cancel_task_action<'de, D>(
    deserializer: D,
) -> Result<Option<SlackCancelTaskAction>, D::Error>
where
    D: Deserializer<'de>,
{
    let actions = Vec::<SlackInteractionAction>::deserialize(deserializer)?;

    Ok(actions.into_iter().find_map(|action| match action {
        SlackInteractionAction::CancelTask(action) => Some(action),
        SlackInteractionAction::Other => None,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use reili_core::messaging::slack::{SlackCancelJobInteraction, SlackInteraction};

    use super::{ParsedSlackInteraction, SlackInteractionForm, parse_slack_interaction_value};

    #[test]
    fn parses_cancel_task_interaction_payload() {
        let parsed = parse_slack_interaction_value(json!({
            "type": "block_actions",
            "user": { "id": "U001" },
            "channel": { "id": "C001" },
            "message": {
                "ts": "1710000000.000002",
                "thread_ts": "1710000000.000001"
            },
            "actions": [
                {
                    "action_id": "cancel_task",
                    "value": "job-1"
                }
            ]
        }))
        .expect("parse interaction");

        assert_eq!(
            parsed,
            ParsedSlackInteraction::Interaction(SlackInteraction::CancelJob(
                SlackCancelJobInteraction {
                    job_id: "job-1".to_string(),
                    user_id: "U001".to_string(),
                    channel: "C001".to_string(),
                    thread_ts: "1710000000.000001".to_string(),
                    message_ts: "1710000000.000002".to_string(),
                }
            ))
        );
    }

    #[test]
    fn parses_form_encoded_interaction_request() {
        let body = serde_urlencoded::to_string([(
            "payload",
            json!({
                "type": "block_actions",
                "user": { "id": "U001" },
                "channel": { "id": "C001" },
                "message": { "ts": "1710000000.000002" },
                "actions": [{ "action_id": "cancel_task", "value": "job-1" }]
            })
            .to_string(),
        )])
        .expect("serialize interaction body");

        let parsed = serde_urlencoded::from_str::<SlackInteractionForm>(&body)
            .expect("parse request")
            .payload;

        assert!(matches!(
            parsed,
            ParsedSlackInteraction::Interaction(SlackInteraction::CancelJob(_))
        ));
    }

    #[test]
    fn ignores_non_cancel_actions() {
        let parsed = parse_slack_interaction_value(json!({
            "type": "block_actions",
            "user": { "id": "U001" },
            "channel": { "id": "C001" },
            "message": { "ts": "1710000000.000002" },
            "actions": [
                {
                    "action_id": "something_else",
                    "value": "job-1"
                }
            ]
        }))
        .expect("parse interaction");

        assert_eq!(parsed, ParsedSlackInteraction::Ignored);
    }
}
