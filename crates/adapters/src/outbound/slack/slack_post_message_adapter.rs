use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{SlackPostMessageInput, SlackPostMessagePort};
use serde_json::json;

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackPostMessageAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackPostMessageAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackPostMessagePort for SlackPostMessageAdapter {
    async fn post_message(&self, input: SlackPostMessageInput) -> Result<(), PortError> {
        let mut body = json!({
            "channel": input.channel,
            "markdown_text": input.text,
            "unfurl_links": false,
        });
        if let Some(thread_ts) = input.thread_ts {
            body["thread_ts"] = json!(thread_ts);
        }
        self.client.post("chat.postMessage", &body).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{SlackPostMessageInput, SlackPostMessagePort};
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackPostMessageAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn posts_to_the_channel_without_a_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({
                "channel": "C123",
                "markdown_text": "report",
                "unfurl_links": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1710000000.000010",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = SlackPostMessageAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .post_message(SlackPostMessageInput {
                channel: "C123".to_string(),
                thread_ts: None,
                text: "report".to_string(),
            })
            .await
            .expect("post channel message");
    }

    #[tokio::test]
    async fn posts_thread_reply_via_chat_post_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({
                "channel": "C123",
                "thread_ts": "1710000000.000001",
                "markdown_text": "reply",
                "unfurl_links": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1710000000.000010",
            })))
            .mount(&server)
            .await;

        let adapter = SlackPostMessageAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .post_message(SlackPostMessageInput {
                channel: "C123".to_string(),
                thread_ts: Some("1710000000.000001".to_string()),
                text: "reply".to_string(),
            })
            .await
            .expect("post thread reply");
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
