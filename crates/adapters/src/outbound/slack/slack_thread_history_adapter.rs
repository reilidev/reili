use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
use reili_core::messaging::slack::{
    SlackLegacyAttachment, SlackMessageFile, SlackMessageMetadata, SlackThreadMessage,
    render_slack_legacy_attachments_text,
};
use serde::Serialize;
use serde_json::Value;

use super::slack_web_api_client::SlackWebApiClient;
use crate::json_utils::read_non_empty_json_string;

const THREAD_HISTORY_PAGE_LIMIT: usize = 100;
const THREAD_HISTORY_MAX_MESSAGES: usize = 200;

#[derive(Debug, Clone)]
pub struct SlackThreadHistoryAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
struct ConversationsRepliesQuery<'a> {
    channel: &'a str,
    ts: &'a str,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
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
            let response = self
                .client
                .get(
                    "conversations.replies",
                    &ConversationsRepliesQuery {
                        channel: &channel,
                        ts: &thread_ts,
                        limit: THREAD_HISTORY_PAGE_LIMIT,
                        cursor: cursor.as_deref(),
                    },
                )
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
                let legacy_attachments = read_legacy_attachments(page_message.get("attachments"));
                let files = read_message_files(page_message.get("files"));

                messages.push(SlackThreadMessage {
                    ts,
                    user: read_non_empty_json_string(page_message.get("user")),
                    text: read_non_empty_json_string(page_message.get("text"))
                        .or_else(|| render_slack_legacy_attachments_text(&legacy_attachments))
                        .unwrap_or_default(),
                    legacy_attachments,
                    files,
                    metadata: read_message_metadata(page_message.get("metadata")),
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

fn read_message_metadata(value: Option<&Value>) -> Option<SlackMessageMetadata> {
    let value = value?;
    let event_type = read_non_empty_json_string(value.get("event_type"))?;
    Some(SlackMessageMetadata {
        event_type,
        event_payload: value.get("event_payload").cloned().unwrap_or(Value::Null),
    })
}

fn read_legacy_attachments(value: Option<&Value>) -> Vec<SlackLegacyAttachment> {
    let Some(value) = value else {
        return Vec::new();
    };

    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn read_message_files(value: Option<&Value>) -> Vec<SlackMessageFile> {
    let Some(value) = value else {
        return Vec::new();
    };

    serde_json::from_value(value.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{
        FetchSlackThreadHistoryInput, SlackLegacyAttachment, SlackLegacyAttachmentField,
        SlackMessageFile, SlackThreadHistoryPort, SlackThreadMessage,
    };
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{SlackThreadHistoryAdapter, THREAD_HISTORY_PAGE_LIMIT};
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn merges_paginated_replies_in_returned_order() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000000"))
            .and(query_param("limit", THREAD_HISTORY_PAGE_LIMIT.to_string()))
            .and(query_param_is_missing("cursor"))
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
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000000"))
            .and(query_param("limit", THREAD_HISTORY_PAGE_LIMIT.to_string()))
            .and(query_param("cursor", "cursor-2"))
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
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: Some("U2".to_string()),
                    text: "second".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
                SlackThreadMessage {
                    ts: "1710000000.000003".to_string(),
                    user: Some("U3".to_string()),
                    text: "third".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
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

        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000000"))
            .and(query_param("limit", THREAD_HISTORY_PAGE_LIMIT.to_string()))
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

    #[tokio::test]
    async fn falls_back_to_legacy_attachment_text_when_reply_text_is_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000000"))
            .and(query_param("limit", THREAD_HISTORY_PAGE_LIMIT.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1710000000.000001",
                        "user": "U1",
                        "text": "",
                        "attachments": [
                            {
                                "title": "Alert",
                                "text": "Disk usage above threshold",
                                "fields": [
                                    {
                                        "title": "Host",
                                        "value": "db-1"
                                    }
                                ]
                            }
                        ]
                    }
                ]
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
            vec![SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U1".to_string()),
                text: "<None|Alert|> Disk usage above threshold".to_string(),
                legacy_attachments: vec![SlackLegacyAttachment {
                    title: Some("Alert".to_string()),
                    text: Some("Disk usage above threshold".to_string()),
                    fields: vec![SlackLegacyAttachmentField {
                        title: Some("Host".to_string()),
                        value: Some("db-1".to_string()),
                        short: None,
                    }],
                    ..Default::default()
                }],
                files: Vec::new(),
                metadata: None,
            }]
        );
    }

    #[tokio::test]
    async fn reads_attached_file_name_and_plain_text_from_replies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000000"))
            .and(query_param("limit", THREAD_HISTORY_PAGE_LIMIT.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1710000000.000001",
                        "user": "U1",
                        "text": "",
                        "files": [{
                            "name": "aws-health.eml",
                            "title": "AWS Health Event",
                            "plain_text": "scheduled upgrade required"
                        }]
                    }
                ]
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
            vec![SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U1".to_string()),
                text: String::new(),
                legacy_attachments: Vec::new(),
                files: vec![SlackMessageFile {
                    name: Some("aws-health.eml".to_string()),
                    title: Some("AWS Health Event".to_string()),
                    plain_text: Some("scheduled upgrade required".to_string()),
                }],
                metadata: None,
            }]
        );
        assert_eq!(
            result[0].rendered_text(),
            "attached_file: aws-health.eml\nplain_text:\nscheduled upgrade required"
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
