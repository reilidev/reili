use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackFileSharedEvent, SlackFileSharedMessagePort, SlackMessage, SlackMessageFile,
    SlackTriggerType,
};
use serde::{Deserialize, Serialize};

use super::slack_web_api_client::SlackWebApiClient;
use crate::json_utils::truncate_for_error;

#[derive(Debug, Clone)]
pub struct SlackFileSharedMessageAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
struct FilesInfoQuery<'a> {
    file: &'a str,
}

#[derive(Debug, Deserialize)]
struct FilesInfoResponse {
    file: SlackFileInfo,
}

#[derive(Debug, Deserialize)]
struct SlackFileInfo {
    id: String,
    name: Option<String>,
    title: Option<String>,
    mimetype: Option<String>,
    url_private_download: Option<String>,
    size: Option<u64>,
    bot_id: Option<String>,
    bot_user_id: Option<String>,
    #[serde(default)]
    display_as_bot: bool,
    text: Option<String>,
    plain_text: Option<String>,
    contents: Option<String>,
    preview_plain_text: Option<String>,
    #[serde(default)]
    shares: SlackFileShares,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SlackFileShares {
    #[serde(flatten)]
    by_visibility: BTreeMap<String, BTreeMap<String, Vec<SlackFileShare>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct SlackFileShare {
    ts: Option<String>,
    thread_ts: Option<String>,
}

impl SlackFileSharedMessageAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }

    fn resolve_file_content(file: &SlackFileInfo) -> ResolvedSlackFileContent {
        non_empty_string(file.text.as_deref())
            .or_else(|| non_empty_string(file.plain_text.as_deref()))
            .or_else(|| non_empty_string(file.contents.as_deref()))
            .or_else(|| non_empty_string(file.preview_plain_text.as_deref()))
            .map(ResolvedSlackFileContent::Text)
            .unwrap_or(ResolvedSlackFileContent::Binary)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedSlackFileContent {
    Text(String),
    Binary,
}

#[async_trait]
impl SlackFileSharedMessagePort for SlackFileSharedMessageAdapter {
    async fn fetch_shared_file_message(
        &self,
        event: SlackFileSharedEvent,
    ) -> Result<Option<SlackMessage>, PortError> {
        let response = self
            .client
            .get(
                "files.info",
                &FilesInfoQuery {
                    file: &event.file_id,
                },
            )
            .await?;
        let parsed: FilesInfoResponse = serde_json::from_value(response).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse Slack files.info response JSON: {error}"
            ))
        })?;
        let file = parsed.file;
        let share = file.shares.find_share(&event.channel_id);
        let ts = share
            .as_ref()
            .and_then(|share| non_empty_string(share.ts.as_deref()))
            .unwrap_or_else(|| event.event_ts.clone());

        let file_content = Self::resolve_file_content(&file);
        match &file_content {
            ResolvedSlackFileContent::Text(text) => {
                tracing::debug!(
                    slack_event_id = %event.slack_event_id,
                    file_id = %file.id,
                    file_content_kind = "text",
                    file_text = %truncate_for_error(text),
                    "Resolved Slack shared file content"
                );
            }
            ResolvedSlackFileContent::Binary => {
                tracing::debug!(
                    slack_event_id = %event.slack_event_id,
                    file_id = %file.id,
                    file_content_kind = "binary",
                    "Resolved Slack shared file content"
                );
            }
        }
        let (plain_text, is_binary) = match file_content {
            ResolvedSlackFileContent::Text(text) => (Some(text), false),
            ResolvedSlackFileContent::Binary => (None, true),
        };

        Ok(Some(SlackMessage {
            slack_event_id: event.slack_event_id,
            team_id: Some(event.team_id),
            action_token: None,
            trigger: SlackTriggerType::Message,
            channel: event.channel_id,
            user: event.user_id,
            actor_is_bot: file.display_as_bot
                || file.bot_id.is_some()
                || file.bot_user_id.is_some(),
            text: String::new(),
            legacy_attachments: Vec::new(),
            files: vec![SlackMessageFile {
                name: file.name,
                title: file.title,
                plain_text,
                is_binary,
                mimetype: non_empty_string(file.mimetype.as_deref()),
                download_url: non_empty_string(file.url_private_download.as_deref()),
                size: file.size,
            }],
            ts,
            thread_ts: share
                .as_ref()
                .and_then(|share| non_empty_string(share.thread_ts.as_deref())),
        }))
    }
}

impl SlackFileShares {
    fn find_share(&self, channel_id: &str) -> Option<ResolvedSlackFileShare> {
        self.by_visibility
            .values()
            .find_map(|shares_by_channel| shares_by_channel.get(channel_id))
            .and_then(|shares| shares.first())
            .map(ResolvedSlackFileShare::from_share)
    }
}

#[derive(Debug, Clone)]
struct ResolvedSlackFileShare {
    ts: Option<String>,
    thread_ts: Option<String>,
}

impl ResolvedSlackFileShare {
    fn from_share(share: &SlackFileShare) -> Self {
        Self {
            ts: share.ts.clone(),
            thread_ts: share.thread_ts.clone(),
        }
    }
}

fn non_empty_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{
        SlackFileSharedEvent, SlackFileSharedMessagePort, SlackMessageFile, SlackTriggerType,
    };
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackFileSharedMessageAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn converts_file_shared_event_with_plain_text_to_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "F001",
                    "name": "alert.eml",
                    "title": "AWS Health Event",
                    "user": "U001",
                    "plain_text": "scheduled upgrade required",
                    "shares": {
                        "public": {
                            "C001": [
                                {
                                    "ts": "1710000000.000100",
                                    "user": "U001",
                                    "team_id": "T001"
                                }
                            ]
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackFileSharedMessageAdapter::new(Arc::new(create_client(&server.uri())));
        let message = adapter
            .fetch_shared_file_message(create_event())
            .await
            .expect("fetch shared file message")
            .expect("message");

        assert_eq!(message.slack_event_id, "Ev-file");
        assert_eq!(message.team_id, Some("T001".to_string()));
        assert_eq!(message.trigger, SlackTriggerType::Message);
        assert_eq!(message.channel, "C001");
        assert_eq!(message.user, "U001");
        assert_eq!(message.ts, "1710000000.000100");
        assert_eq!(
            message.files,
            vec![SlackMessageFile {
                name: Some("alert.eml".to_string()),
                title: Some("AWS Health Event".to_string()),
                plain_text: Some("scheduled upgrade required".to_string()),
                is_binary: false,
                ..Default::default()
            }]
        );
        assert_eq!(
            message.rendered_text(),
            "## Attached file title\n alert.eml\n\n## Plain text\nscheduled upgrade required\n"
        );
    }

    #[tokio::test]
    async fn marks_file_as_binary_when_slack_text_is_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "F001",
                    "name": "alert.eml",
                    "title": "AWS Health Event",
                    "user": "U001",
                    "shares": {
                        "public": {
                            "C001": [
                                {
                                    "ts": "1710000000.000100",
                                    "user": "U001",
                                    "team_id": "T001"
                                }
                            ]
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackFileSharedMessageAdapter::new(Arc::new(create_client(&server.uri())));
        let message = adapter
            .fetch_shared_file_message(create_event())
            .await
            .expect("fetch shared file message")
            .expect("message");

        assert_eq!(message.files[0].plain_text, None);
        assert!(message.files[0].is_binary);
        assert_eq!(
            message.rendered_text(),
            "## Attached file title\n alert.eml\n\nThis is binary file"
        );
    }

    #[tokio::test]
    async fn captures_pdf_mimetype_and_download_url_as_binary_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/files.info"))
            .and(query_param("file", "F001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "F001",
                    "name": "incident-report.pdf",
                    "title": "Incident Report",
                    "mimetype": "application/pdf",
                    "url_private_download": "https://files.slack.com/files-pri/T001-F001/download/incident-report.pdf",
                    "size": 204_800,
                    "user": "U001",
                    "shares": {
                        "public": {
                            "C001": [
                                {
                                    "ts": "1710000000.000100",
                                    "user": "U001",
                                    "team_id": "T001"
                                }
                            ]
                        }
                    }
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackFileSharedMessageAdapter::new(Arc::new(create_client(&server.uri())));
        let message = adapter
            .fetch_shared_file_message(create_event())
            .await
            .expect("fetch shared file message")
            .expect("message");

        let file = &message.files[0];
        assert!(file.is_binary);
        assert_eq!(file.mimetype.as_deref(), Some("application/pdf"));
        assert_eq!(file.size, Some(204_800));
        assert!(file.is_pdf());
        assert_eq!(
            file.pdf_download_url(),
            Some("https://files.slack.com/files-pri/T001-F001/download/incident-report.pdf")
        );
    }

    fn create_event() -> SlackFileSharedEvent {
        SlackFileSharedEvent {
            slack_event_id: "Ev-file".to_string(),
            team_id: "T001".to_string(),
            channel_id: "C001".to_string(),
            file_id: "F001".to_string(),
            user_id: "U001".to_string(),
            event_ts: "1710000000.000010".to_string(),
        }
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
