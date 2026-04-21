use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{PostSlackEphemeralMessageInput, SlackEphemeralMessagePort};
use serde::Serialize;

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackEphemeralMessageAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct ChatPostEphemeralRequest {
    channel: String,
    user: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<String>,
}

impl SlackEphemeralMessageAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackEphemeralMessagePort for SlackEphemeralMessageAdapter {
    async fn post_ephemeral_message(
        &self,
        input: PostSlackEphemeralMessageInput,
    ) -> Result<(), PortError> {
        let request = ChatPostEphemeralRequest {
            channel: input.channel,
            user: input.user,
            text: input.text,
            thread_ts: input.thread_ts,
        };

        self.client.post("chat.postEphemeral", &request).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{PostSlackEphemeralMessageInput, SlackEphemeralMessagePort};
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackEphemeralMessageAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn posts_ephemeral_message_with_channel_user_thread_ts_and_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .and(body_json(json!({
                "channel": "C123",
                "user": "U123",
                "thread_ts": "1710000000.000001",
                "text": "not authorized",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "message_ts": "1710000000.000010",
            })))
            .mount(&server)
            .await;

        let adapter = SlackEphemeralMessageAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .post_ephemeral_message(PostSlackEphemeralMessageInput {
                channel: "C123".to_string(),
                user: "U123".to_string(),
                thread_ts: Some("1710000000.000001".to_string()),
                text: "not authorized".to_string(),
            })
            .await
            .expect("post ephemeral message");
    }

    #[tokio::test]
    async fn omits_thread_ts_when_posting_channel_ephemeral_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postEphemeral"))
            .and(body_json(json!({
                "channel": "C123",
                "user": "U123",
                "text": "not authorized",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "message_ts": "1710000000.000010",
            })))
            .mount(&server)
            .await;

        let adapter = SlackEphemeralMessageAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .post_ephemeral_message(PostSlackEphemeralMessageInput {
                channel: "C123".to_string(),
                user: "U123".to_string(),
                thread_ts: None,
                text: "not authorized".to_string(),
            })
            .await
            .expect("post ephemeral message");
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
