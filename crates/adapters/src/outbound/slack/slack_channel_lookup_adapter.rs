use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{SlackChannelInfo, SlackChannelLookupPort};
use serde::{Deserialize, Serialize};

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackChannelLookupAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
struct ConversationsInfoQuery<'a> {
    channel: &'a str,
}

#[derive(Debug, Deserialize)]
struct ConversationsInfoResponse {
    channel: ConversationsInfoChannel,
}

#[derive(Debug, Deserialize)]
struct ConversationsInfoChannel {
    name: String,
    is_private: bool,
}

impl SlackChannelLookupAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackChannelLookupPort for SlackChannelLookupAdapter {
    async fn lookup_channel_info(&self, channel_id: &str) -> Result<SlackChannelInfo, PortError> {
        let channel_id = channel_id.trim();
        if channel_id.is_empty() {
            return Err(PortError::invalid_input(
                "Slack channel ID must not be empty",
            ));
        }

        let response = self
            .client
            .get(
                "conversations.info",
                &ConversationsInfoQuery {
                    channel: channel_id,
                },
            )
            .await?;

        let parsed: ConversationsInfoResponse =
            serde_json::from_value(response).map_err(|error| {
                PortError::invalid_response(format!(
                    "Failed to parse Slack conversations.info response JSON: {error}"
                ))
            })?;
        let name = parsed.channel.name.trim().to_string();
        if name.is_empty() {
            return Err(PortError::invalid_response(
                "Slack conversations.info response did not contain channel.name",
            ));
        }

        Ok(SlackChannelInfo {
            name,
            is_private: parsed.channel.is_private,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{SlackChannelInfo, SlackChannelLookupPort};
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackChannelLookupAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn reads_channel_info_from_conversations_info_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(query_param("channel", "C123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": {
                    "id": "C123",
                    "name": "alerts-prod",
                    "is_private": false
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackChannelLookupAdapter::new(Arc::new(create_client(&server.uri())));
        let channel_info = adapter
            .lookup_channel_info("C123")
            .await
            .expect("lookup channel info");

        assert_eq!(
            channel_info,
            SlackChannelInfo {
                name: "alerts-prod".to_string(),
                is_private: false,
            }
        );
    }

    #[tokio::test]
    async fn reads_private_channel_flag_from_conversations_info_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(query_param("channel", "C-private"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": {
                    "id": "C-private",
                    "name": "private-alerts",
                    "is_private": true
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackChannelLookupAdapter::new(Arc::new(create_client(&server.uri())));
        let channel_info = adapter
            .lookup_channel_info("C-private")
            .await
            .expect("lookup channel info");

        assert_eq!(
            channel_info,
            SlackChannelInfo {
                name: "private-alerts".to_string(),
                is_private: true,
            }
        );
    }

    #[tokio::test]
    async fn rejects_response_when_channel_name_is_blank() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.info"))
            .and(query_param("channel", "C123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": {
                    "id": "C123",
                    "name": " ",
                    "is_private": false
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackChannelLookupAdapter::new(Arc::new(create_client(&server.uri())));
        let error = adapter
            .lookup_channel_info("C123")
            .await
            .expect_err("blank channel name should fail");

        assert!(error.message.contains("channel.name"));
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
