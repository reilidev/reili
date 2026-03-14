use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::error::PortError;
use reili_shared::messaging::slack::{SlackThreadReplyInput, SlackThreadReplyPort};
use serde_json::json;

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackThreadReplyAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackThreadReplyAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackThreadReplyPort for SlackThreadReplyAdapter {
    async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError> {
        self.client
            .post(
                "chat.postMessage",
                &json!({
                    "channel": input.channel,
                    "thread_ts": input.thread_ts,
                    "markdown_text": input.text,
                }),
            )
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_shared::messaging::slack::{SlackThreadReplyInput, SlackThreadReplyPort};
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackThreadReplyAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn posts_thread_reply_via_chat_post_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_json(json!({
                "channel": "C123",
                "thread_ts": "1710000000.000001",
                "markdown_text": "reply",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1710000000.000010",
            })))
            .mount(&server)
            .await;

        let adapter = SlackThreadReplyAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .post_thread_reply(SlackThreadReplyInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                text: "reply".to_string(),
            })
            .await
            .expect("post thread reply");
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
