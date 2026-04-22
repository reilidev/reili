use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    PostTaskControlMessageInput, PostTaskControlMessageOutput, SlackTaskControlMessagePort,
    SlackTaskControlState, UpdateTaskControlMessageInput,
};
use serde_json::json;

use super::slack_web_api_client::SlackWebApiClient;
use crate::json_utils::read_non_empty_json_string;

const CONTROL_MESSAGE_EVENT_TYPE: &str = "task_control_message_posted";
const CONTROL_MESSAGE_KIND: &str = "cancel_control";

#[derive(Debug, Clone)]
pub struct SlackTaskControlMessageAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackTaskControlMessageAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackTaskControlMessagePort for SlackTaskControlMessageAdapter {
    async fn post_task_control_message(
        &self,
        input: PostTaskControlMessageInput,
    ) -> Result<PostTaskControlMessageOutput, PortError> {
        let response = self
            .client
            .post(
                "chat.postMessage",
                &json!({
                    "channel": input.channel,
                    "thread_ts": input.thread_ts,
                    "text": render_control_text(&input.state),
                    "blocks": build_control_blocks(&input.state, &input.job_id),
                    "metadata": build_control_metadata(&input.job_id, &input.thread_ts),
                }),
            )
            .await?;

        let message_ts = read_non_empty_json_string(response.get("ts")).ok_or_else(|| {
            PortError::new("Slack control message post response did not contain ts")
        })?;

        Ok(PostTaskControlMessageOutput { message_ts })
    }

    async fn update_task_control_message(
        &self,
        input: UpdateTaskControlMessageInput,
    ) -> Result<(), PortError> {
        self.client
            .post(
                "chat.update",
                &json!({
                    "channel": input.channel,
                    "ts": input.message_ts,
                    "text": render_control_text(&input.state),
                    "blocks": build_control_blocks(&input.state, &input.job_id),
                    "metadata": build_control_metadata(&input.job_id, &input.thread_ts),
                }),
            )
            .await?;

        Ok(())
    }
}

fn build_control_metadata(job_id: &str, thread_ts: &str) -> serde_json::Value {
    json!({
        "event_type": CONTROL_MESSAGE_EVENT_TYPE,
        "event_payload": {
            "job_id": job_id,
            "thread_ts": thread_ts,
            "kind": CONTROL_MESSAGE_KIND,
        }
    })
}

fn render_control_text(state: &SlackTaskControlState) -> String {
    match state {
        SlackTaskControlState::Queued => "Task queued.".to_string(),
        SlackTaskControlState::Running => "Task is in progress.".to_string(),
        SlackTaskControlState::CancellationRequested {
            requested_by_user_id,
        } => {
            if requested_by_user_id.is_empty() {
                "Cancellation requested. Stopping the task...".to_string()
            } else {
                format!("Cancellation requested by <@{requested_by_user_id}>. Stopping the task...")
            }
        }
        SlackTaskControlState::Cancelled {
            cancelled_by_user_id,
        } => {
            if cancelled_by_user_id.is_empty() {
                "Task cancelled.".to_string()
            } else {
                format!("Task cancelled by <@{cancelled_by_user_id}>.")
            }
        }
        SlackTaskControlState::Completed => "Task completed.".to_string(),
        SlackTaskControlState::Failed => "Task failed.".to_string(),
    }
}

fn build_control_blocks(state: &SlackTaskControlState, job_id: &str) -> Vec<serde_json::Value> {
    let section = json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": render_control_text(state),
        }
    });

    if matches!(
        state,
        SlackTaskControlState::Queued | SlackTaskControlState::Running
    ) {
        return vec![
            section,
            json!({
                "type": "actions",
                "elements": [
                    {
                        "type": "button",
                        "action_id": "cancel_task",
                        "text": {
                            "type": "plain_text",
                            "text": "Cancel",
                        },
                        "style": "danger",
                        "value": job_id,
                    }
                ]
            }),
        ];
    }

    vec![section]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use reili_core::messaging::slack::{
        PostTaskControlMessageInput, SlackTaskControlMessagePort, SlackTaskControlState,
        UpdateTaskControlMessageInput,
    };
    use reili_core::secret::SecretString;

    use super::SlackTaskControlMessageAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn posts_control_message_with_cancel_button_and_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({
                "channel": "C123",
                "thread_ts": "1710000000.000001",
                "text": "Task queued.",
                "blocks": [
                    {
                        "type": "section",
                        "text": {
                            "type": "mrkdwn",
                            "text": "Task queued."
                        }
                    },
                    {
                        "type": "actions",
                        "elements": [
                            {
                                "type": "button",
                                "action_id": "cancel_task",
                                "text": {
                                    "type": "plain_text",
                                    "text": "Cancel"
                                },
                                "style": "danger",
                                "value": "job-1"
                            }
                        ]
                    }
                ],
                "metadata": {
                    "event_type": "task_control_message_posted",
                    "event_payload": {
                        "job_id": "job-1",
                        "thread_ts": "1710000000.000001",
                        "kind": "cancel_control"
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1710000000.000002"
            })))
            .mount(&server)
            .await;

        let adapter = SlackTaskControlMessageAdapter::new(Arc::new(create_client(&server.uri())));
        let result = adapter
            .post_task_control_message(PostTaskControlMessageInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                job_id: "job-1".to_string(),
                state: SlackTaskControlState::Queued,
            })
            .await
            .expect("post control message");

        assert_eq!(result.message_ts, "1710000000.000002");
    }

    #[tokio::test]
    async fn updates_control_message_without_cancel_button_in_terminal_state() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.update"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000002",
                "text": "Task completed.",
                "blocks": [
                    {
                        "type": "section",
                        "text": {
                            "type": "mrkdwn",
                            "text": "Task completed."
                        }
                    }
                ],
                "metadata": {
                    "event_type": "task_control_message_posted",
                    "event_payload": {
                        "job_id": "job-1",
                        "thread_ts": "1710000000.000001",
                        "kind": "cancel_control"
                    }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let adapter = SlackTaskControlMessageAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .update_task_control_message(UpdateTaskControlMessageInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                message_ts: "1710000000.000002".to_string(),
                job_id: "job-1".to_string(),
                state: SlackTaskControlState::Completed,
            })
            .await
            .expect("update control message");
    }

    #[test]
    fn renders_task_oriented_control_text() {
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Queued),
            "Task queued."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Running),
            "Task is in progress."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::CancellationRequested {
                requested_by_user_id: String::new(),
            }),
            "Cancellation requested. Stopping the task..."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::CancellationRequested {
                requested_by_user_id: "U123".to_string(),
            }),
            "Cancellation requested by <@U123>. Stopping the task..."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Cancelled {
                cancelled_by_user_id: String::new(),
            }),
            "Task cancelled."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Cancelled {
                cancelled_by_user_id: "U123".to_string(),
            }),
            "Task cancelled by <@U123>."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Completed),
            "Task completed."
        );
        assert_eq!(
            super::render_control_text(&SlackTaskControlState::Failed),
            "Task failed."
        );
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
