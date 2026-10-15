//! Posts goal reports to Slack with message metadata carrying an idempotency key, and finds
//! already-posted reports by scanning recent channel history for that key.
//! Metadata: <https://api.slack.com/reference/metadata>,
//! history: <https://api.slack.com/methods/conversations.history>.

use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{GoalReportMessage, GoalReportPort, PostedGoalReport};
use serde_json::{Value, json};

use super::slack_web_api_client::SlackWebApiClient;

const REPORT_EVENT_TYPE: &str = "reili_goal_report";
const HISTORY_LOOKBACK_MESSAGES: u32 = 100;
/// History lookup failures that mean "cannot check" rather than "Slack is down". In these cases
/// posting proceeds (at-least-once) instead of blocking the cycle forever.
const DEGRADED_LOOKUP_ERRORS: &[&str] = &["missing_scope", "not_in_channel", "channel_not_found"];

pub struct SlackGoalReportAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackGoalReportAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

fn message_report_key(message: &Value) -> Option<&str> {
    let metadata = message.get("metadata")?;
    if metadata.get("event_type")?.as_str()? != REPORT_EVENT_TYPE {
        return None;
    }
    metadata.get("event_payload")?.get("report_key")?.as_str()
}

#[async_trait]
impl GoalReportPort for SlackGoalReportAdapter {
    async fn find_posted_report(
        &self,
        channel_id: &str,
        report_key: &str,
    ) -> Result<Option<PostedGoalReport>, PortError> {
        let response = self
            .client
            .get(
                "conversations.history",
                &[
                    ("channel", channel_id.to_string()),
                    ("limit", HISTORY_LOOKBACK_MESSAGES.to_string()),
                    ("include_all_metadata", "true".to_string()),
                ],
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error)
                if DEGRADED_LOOKUP_ERRORS
                    .iter()
                    .any(|code| error.is_service_error_code(code)) =>
            {
                tracing::warn!(
                    channel_id,
                    error = error.message,
                    "Cannot check for an already-posted goal report; posting may repeat after a crash"
                );
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok(response
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message_report_key(message) == Some(report_key))
            })
            .and_then(|message| message.get("ts").and_then(Value::as_str))
            .map(|ts| PostedGoalReport {
                channel_id: channel_id.to_string(),
                ts: ts.to_string(),
            }))
    }

    async fn post_report(&self, message: GoalReportMessage) -> Result<PostedGoalReport, PortError> {
        let response = self
            .client
            .post(
                "chat.postMessage",
                &json!({
                    "channel": message.channel_id,
                    "markdown_text": message.text,
                    "unfurl_links": false,
                    "metadata": {
                        "event_type": REPORT_EVENT_TYPE,
                        "event_payload": {
                            "report_key": message.report_key,
                            "goal_id": message.goal_id,
                        }
                    }
                }),
            )
            .await?;
        let ts = response
            .get("ts")
            .and_then(Value::as_str)
            .ok_or_else(|| PortError::invalid_response("chat.postMessage response had no ts"))?;
        Ok(PostedGoalReport {
            channel_id: message.channel_id,
            ts: ts.to_string(),
        })
    }

    async fn post_thread_reply(
        &self,
        channel_id: &str,
        thread_ts: &str,
        text: &str,
    ) -> Result<(), PortError> {
        self.client
            .post(
                "chat.postMessage",
                &json!({
                    "channel": channel_id,
                    "thread_ts": thread_ts,
                    "markdown_text": text,
                    "unfurl_links": false,
                }),
            )
            .await?;
        Ok(())
    }

    async fn permalink(&self, channel_id: &str, ts: &str) -> Result<Option<String>, PortError> {
        let response = self
            .client
            .get(
                "chat.getPermalink",
                &[("channel", channel_id), ("message_ts", ts)],
            )
            .await?;
        Ok(response
            .get("permalink")
            .and_then(Value::as_str)
            .map(ToString::to_string))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{GoalReportMessage, GoalReportPort};
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackGoalReportAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    fn adapter(server: &MockServer) -> SlackGoalReportAdapter {
        SlackGoalReportAdapter::new(Arc::new(
            SlackWebApiClient::new(SlackWebApiClientConfig {
                bot_token: SecretString::from("xoxb-test"),
                base_url: Some(server.uri()),
            })
            .expect("client"),
        ))
    }

    #[tokio::test]
    async fn posts_report_with_idempotency_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(body_partial_json(json!({
                "channel": "C1",
                "metadata": {
                    "event_type": "reili_goal_report",
                    "event_payload": { "report_key": "checkout:run-1", "goal_id": "checkout" }
                }
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "ts": "1.2" })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let posted = adapter(&server)
            .post_report(GoalReportMessage {
                channel_id: "C1".to_string(),
                goal_id: "checkout".to_string(),
                report_key: "checkout:run-1".to_string(),
                text: "report".to_string(),
            })
            .await
            .expect("post");

        assert_eq!(posted.ts, "1.2");
    }

    #[tokio::test]
    async fn finds_previously_posted_report_by_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .and(query_param("include_all_metadata", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "3.0", "text": "unrelated" },
                    { "ts": "2.0", "metadata": { "event_type": "reili_goal_report", "event_payload": { "report_key": "checkout:run-1" } } }
                ]
            })))
            .mount(&server)
            .await;

        let adapter = adapter(&server);
        let found = adapter
            .find_posted_report("C1", "checkout:run-1")
            .await
            .expect("lookup");
        let missing = adapter
            .find_posted_report("C1", "checkout:run-2")
            .await
            .expect("lookup");

        assert_eq!(found.map(|report| report.ts), Some("2.0".to_string()));
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn degrades_when_history_scope_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.history"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "missing_scope"
            })))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .find_posted_report("C1", "checkout:run-1")
            .await
            .expect("degraded lookup");

        assert!(found.is_none());
    }
}
