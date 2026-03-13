use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
use reili_shared::types::SlackThreadMessage;
use serde_json::Value;

use super::slack_web_api_client::SlackWebApiClient;
use crate::json_utils::read_non_empty_json_string;

const THREAD_HISTORY_PAGE_LIMIT: usize = 15;
const THREAD_HISTORY_MAX_MESSAGES: usize = 200;

#[derive(Debug, Clone)]
pub struct SlackThreadHistoryAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackThreadHistoryAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackThreadHistoryPort for SlackThreadHistoryAdapter {
    async fn fetch_thread_history(
        &self,
        input: FetchSlackThreadHistoryInput,
    ) -> Result<Vec<SlackThreadMessage>, PortError> {
        let channel = input.channel;
        let thread_ts = input.thread_ts;
        let mut messages: Vec<SlackThreadMessage> = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut payload = serde_json::Map::from_iter([
                ("channel".to_string(), Value::String(channel.clone())),
                ("ts".to_string(), Value::String(thread_ts.clone())),
                (
                    "limit".to_string(),
                    Value::Number(serde_json::Number::from(THREAD_HISTORY_PAGE_LIMIT)),
                ),
            ]);

            if let Some(cursor_value) = cursor.as_ref() {
                payload.insert("cursor".to_string(), Value::String(cursor_value.clone()));
            }

            let response = self
                .client
                .post("conversations.replies", &Value::Object(payload))
                .await?;

            let page_messages = response
                .get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for page_message in page_messages {
                if messages.len() >= THREAD_HISTORY_MAX_MESSAGES {
                    break;
                }

                let ts = match read_non_empty_json_string(page_message.get("ts")) {
                    Some(value) => value,
                    None => continue,
                };

                messages.push(SlackThreadMessage {
                    ts,
                    user: read_non_empty_json_string(page_message.get("user")),
                    text: page_message
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }

            if messages.len() >= THREAD_HISTORY_MAX_MESSAGES {
                break;
            }

            cursor = read_non_empty_json_string(response.pointer("/response_metadata/next_cursor"));
            if cursor.is_none() {
                break;
            }
        }

        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_shared::ports::outbound::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
    use reili_shared::types::SlackThreadMessage;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackThreadHistoryAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn merges_paginated_replies_in_returned_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/conversations.replies"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000000",
                "limit": 15,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1710000000.000001",
                        "user": "U1",
                        "text": "first",
                    }
                ],
                "response_metadata": {
                    "next_cursor": "cursor-2",
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/conversations.replies"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000000",
                "limit": 15,
                "cursor": "cursor-2",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1710000000.000002",
                        "user": "U2",
                        "text": "second",
                    },
                    {
                        "ts": "1710000000.000003",
                        "user": "U3",
                        "text": "third",
                    }
                ],
                "response_metadata": {
                    "next_cursor": "",
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = SlackThreadHistoryAdapter::new(Arc::new(create_client(&server.uri())));
        let result = adapter
            .fetch_thread_history(FetchSlackThreadHistoryInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000000".to_string(),
            })
            .await
            .expect("fetch thread history");

        assert_eq!(
            result,
            vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U1".to_string()),
                    text: "first".to_string(),
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: Some("U2".to_string()),
                    text: "second".to_string(),
                },
                SlackThreadMessage {
                    ts: "1710000000.000003".to_string(),
                    user: Some("U3".to_string()),
                    text: "third".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn caps_history_to_two_hundred_messages() {
        let server = MockServer::start().await;
        let many_messages: Vec<_> = (0..220)
            .map(|index| {
                json!({
                    "ts": format!("1710000000.{index:06}"),
                    "user": "U1",
                    "text": format!("message-{index}"),
                })
            })
            .collect();

        Mock::given(method("POST"))
            .and(path("/conversations.replies"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": many_messages,
                "response_metadata": {
                    "next_cursor": "cursor-next",
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = SlackThreadHistoryAdapter::new(Arc::new(create_client(&server.uri())));
        let result = adapter
            .fetch_thread_history(FetchSlackThreadHistoryInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000000".to_string(),
            })
            .await
            .expect("fetch thread history");

        assert_eq!(result.len(), 200);
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
