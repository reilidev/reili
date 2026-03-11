use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{
    AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort, SlackStreamBlock,
    StartSlackProgressStreamInput, StartSlackProgressStreamOutput, StopSlackProgressStreamInput,
};

use super::slack_web_api_client::SlackWebApiClient;
use crate::json_utils::read_non_empty_json_string;

#[derive(Debug, Clone)]
pub struct SlackProgressStreamAdapter {
    client: Arc<SlackWebApiClient>,
}

impl SlackProgressStreamAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackProgressStreamPort for SlackProgressStreamAdapter {
    async fn start(
        &self,
        input: StartSlackProgressStreamInput,
    ) -> Result<StartSlackProgressStreamOutput, PortError> {
        if input.markdown_text.is_none() && input.chunks.is_none() {
            return Err(PortError::new(
                "Slack stream start requires markdownText or chunks",
            ));
        }

        let response = self
            .client
            .post(
                "chat.startStream",
                &StartSlackProgressStreamRequest {
                    channel: input.channel,
                    thread_ts: input.thread_ts,
                    recipient_user_id: input.recipient_user_id,
                    recipient_team_id: input.recipient_team_id,
                    markdown_text: input.markdown_text,
                    chunks: input.chunks,
                    task_display_mode: "plan",
                },
            )
            .await?;

        let stream_ts = read_non_empty_json_string(response.get("ts"))
            .ok_or_else(|| PortError::new("Slack stream start response did not contain ts"))?;
        Ok(StartSlackProgressStreamOutput { stream_ts })
    }

    async fn append(&self, input: AppendSlackProgressStreamInput) -> Result<(), PortError> {
        self.client
            .post(
                "chat.appendStream",
                &AppendSlackProgressStreamRequest {
                    channel: input.channel,
                    ts: input.stream_ts,
                    markdown_text: input.markdown_text,
                    chunks: input.chunks,
                },
            )
            .await?;

        Ok(())
    }

    async fn stop(&self, input: StopSlackProgressStreamInput) -> Result<(), PortError> {
        self.client
            .post(
                "chat.stopStream",
                &StopSlackProgressStreamRequest {
                    channel: input.channel,
                    ts: input.stream_ts,
                    markdown_text: input.markdown_text,
                    chunks: input.chunks,
                    blocks: input.blocks,
                },
            )
            .await?;

        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct StartSlackProgressStreamRequest {
    channel: String,
    thread_ts: String,
    recipient_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipient_team_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<SlackAnyChunk>>,
    task_display_mode: &'static str,
}

#[derive(Debug, Serialize)]
struct AppendSlackProgressStreamRequest {
    channel: String,
    ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<SlackAnyChunk>>,
}

#[derive(Debug, Serialize)]
struct StopSlackProgressStreamRequest {
    channel: String,
    ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<SlackAnyChunk>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<Vec<SlackStreamBlock>>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use sre_shared::ports::outbound::slack_progress_stream::{
        SlackTaskUpdateChunk, SlackTaskUpdateStatus,
    };
    use sre_shared::ports::outbound::{
        AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort,
        StartSlackProgressStreamInput, StopSlackProgressStreamInput,
    };
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackProgressStreamAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn start_requires_markdown_text_or_chunks() {
        let adapter = SlackProgressStreamAdapter::new(Arc::new(create_client("http://localhost")));
        let error = adapter
            .start(StartSlackProgressStreamInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: None,
                markdown_text: None,
                chunks: None,
            })
            .await
            .expect_err("start should fail");

        assert!(error.message.contains("requires markdownText or chunks"));
    }

    #[tokio::test]
    async fn starts_stream_in_plan_mode_and_returns_stream_ts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.startStream"))
            .and(body_json(json!({
                "channel": "C123",
                "thread_ts": "1710000000.000001",
                "recipient_user_id": "U123",
                "markdown_text": "Working on it",
                "task_display_mode": "plan",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "ts": "1710000000.000100",
            })))
            .mount(&server)
            .await;

        let adapter = SlackProgressStreamAdapter::new(Arc::new(create_client(&server.uri())));
        let result = adapter
            .start(StartSlackProgressStreamInput {
                channel: "C123".to_string(),
                thread_ts: "1710000000.000001".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: None,
                markdown_text: Some("Working on it".to_string()),
                chunks: None,
            })
            .await
            .expect("start stream");

        assert_eq!(result.stream_ts, "1710000000.000100");
    }

    #[tokio::test]
    async fn append_and_stop_call_stream_apis() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.appendStream"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000100",
                "markdown_text": "progress",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat.stopStream"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000100",
                "markdown_text": "done",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let adapter = SlackProgressStreamAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .append(AppendSlackProgressStreamInput {
                channel: "C123".to_string(),
                stream_ts: "1710000000.000100".to_string(),
                markdown_text: Some("progress".to_string()),
                chunks: None,
            })
            .await
            .expect("append stream");
        adapter
            .stop(StopSlackProgressStreamInput {
                channel: "C123".to_string(),
                stream_ts: "1710000000.000100".to_string(),
                markdown_text: Some("done".to_string()),
                chunks: None,
                blocks: None,
            })
            .await
            .expect("stop stream");
    }

    #[tokio::test]
    async fn append_task_update_omits_none_optional_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.appendStream"))
            .and(body_json(json!({
                "channel": "C123",
                "ts": "1710000000.000100",
                "chunks": [
                    {
                        "type": "task_update",
                        "id": "reasoning-1",
                        "title": "Collect evidence",
                        "status": "in_progress",
                    }
                ],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let adapter = SlackProgressStreamAdapter::new(Arc::new(create_client(&server.uri())));
        adapter
            .append(AppendSlackProgressStreamInput {
                channel: "C123".to_string(),
                stream_ts: "1710000000.000100".to_string(),
                markdown_text: None,
                chunks: Some(vec![SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
                    id: "reasoning-1".to_string(),
                    title: "Collect evidence".to_string(),
                    status: SlackTaskUpdateStatus::InProgress,
                    details: None,
                    output: None,
                    sources: None,
                })]),
            })
            .await
            .expect("append stream");
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
