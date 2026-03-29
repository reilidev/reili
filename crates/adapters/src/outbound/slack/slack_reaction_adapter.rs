use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{AddSlackReactionInput, SlackReactionPort};
use serde_json::json;

use super::slack_web_api_client::SlackWebApiClient;

#[derive(Debug, Clone)]
pub struct SlackReactionAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackReactionAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackReactionPort for SlackReactionAdapter {
    async fn add_reaction(&self, input: AddSlackReactionInput) -> Result<(), PortError> {
        self.client
            .post(
                "reactions.add",
                &json!({
                    "channel": input.channel,
                    "timestamp": input.message_ts,
                    "name": input.name,
                }),
            )
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{AddSlackReactionInput, SlackReactionPort};
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackReactionAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn adds_reaction_via_reactions_add() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/reactions.add"))
            .and(body_json(json!({
                "channel": "C123",
                "timestamp": "1710000000.000001",
                "name": "eyes",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
            })))
            .mount(&server)
            .await;

        let adapter = SlackReactionAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .add_reaction(AddSlackReactionInput {
                channel: "C123".to_string(),
                message_ts: "1710000000.000001".to_string(),
                name: "eyes".to_string(),
            })
            .await
            .expect("add reaction");
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
