use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackContextMessage, SlackMessageSearchContextMessages, SlackMessageSearchInput,
    SlackMessageSearchPort, SlackMessageSearchResult, SlackMessageSearchResultItem,
};
use serde::{Deserialize, Serialize};

use super::slack_web_api_client::SlackWebApiClient;

const MAX_SEARCH_RESULTS: u32 = 5;

#[derive(Debug, Clone)]
pub struct SlackMessageSearchAdapter {
    client: Arc<SlackWebApiClient>,
}

#[derive(Debug, Serialize)]
struct AssistantSearchContextRequest<'a> {
    query: &'a str,
    action_token: &'a str,
    content_types: [&'static str; 1],
    channel_types: [&'static str; 1],
    include_bots: bool,
    include_context_messages: bool,
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<i64>,
    sort: &'a reili_core::messaging::slack::SlackMessageSearchSort,
    #[serde(rename = "sort_dir")]
    sort_direction: &'a reili_core::messaging::slack::SlackMessageSearchSortDirection,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantSearchContextResponse {
    #[serde(default)]
    results: AssistantSearchContextResults,
    #[serde(default)]
    response_metadata: AssistantSearchResponseMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantSearchContextResults {
    #[serde(default)]
    messages: Vec<AssistantSearchMessageDto>,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantSearchResponseMetadata {
    #[serde(default)]
    next_cursor: String,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantSearchMessageDto {
    author_name: Option<String>,
    author_user_id: Option<String>,
    team_id: Option<String>,
    channel_id: Option<String>,
    channel_name: Option<String>,
    message_ts: Option<String>,
    thread_ts: Option<String>,
    content: Option<String>,
    is_author_bot: Option<bool>,
    permalink: Option<String>,
    #[serde(default)]
    context_messages: AssistantSearchContextMessagesDto,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantSearchContextMessagesDto {
    #[serde(default)]
    before: Vec<AssistantContextMessageDto>,
    #[serde(default)]
    after: Vec<AssistantContextMessageDto>,
}

#[derive(Debug, Default, Deserialize)]
struct AssistantContextMessageDto {
    author_name: Option<String>,
    user_id: Option<String>,
    ts: Option<String>,
    text: Option<String>,
}

impl SlackMessageSearchAdapter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SlackMessageSearchPort for SlackMessageSearchAdapter {
    async fn search_messages(
        &self,
        input: SlackMessageSearchInput,
    ) -> Result<SlackMessageSearchResult, PortError> {
        validate_input(&input)?;

        let query = input.query.trim().to_string();
        let action_token = input.action_token.trim().to_string();
        let response = self
            .client
            .post(
                "assistant.search.context",
                &AssistantSearchContextRequest {
                    query: &query,
                    action_token: &action_token,
                    content_types: ["messages"],
                    channel_types: ["public_channel"],
                    include_bots: input.include_bots,
                    include_context_messages: input.include_context_messages,
                    limit: input.limit,
                    before: input.before,
                    after: input.after,
                    sort: &input.sort,
                    sort_direction: &input.sort_direction,
                },
            )
            .await?;

        let parsed: AssistantSearchContextResponse =
            serde_json::from_value(response).map_err(|error| {
                PortError::invalid_response(format!(
                    "Failed to parse Slack search response JSON: {error}"
                ))
            })?;

        Ok(SlackMessageSearchResult {
            messages: parsed
                .results
                .messages
                .into_iter()
                .filter_map(map_message_result)
                .collect(),
            next_cursor: trim_optional_string(Some(parsed.response_metadata.next_cursor)),
        })
    }
}

fn validate_input(input: &SlackMessageSearchInput) -> Result<(), PortError> {
    if input.query.trim().is_empty() {
        return Err(PortError::invalid_input(
            "Slack search query must not be empty",
        ));
    }

    if input.action_token.trim().is_empty() {
        return Err(PortError::invalid_input(
            "Slack search action token must not be empty",
        ));
    }

    if input.limit == 0 || input.limit > MAX_SEARCH_RESULTS {
        return Err(PortError::invalid_input(format!(
            "Slack search limit must be between 1 and {MAX_SEARCH_RESULTS}"
        )));
    }

    if let (Some(after), Some(before)) = (input.after, input.before)
        && after > before
    {
        return Err(PortError::invalid_input(
            "Slack search after timestamp must be less than or equal to before timestamp",
        ));
    }

    Ok(())
}

fn map_message_result(value: AssistantSearchMessageDto) -> Option<SlackMessageSearchResultItem> {
    Some(SlackMessageSearchResultItem {
        author_name: trim_optional_string(value.author_name),
        author_user_id: trim_optional_string(value.author_user_id),
        team_id: trim_optional_string(value.team_id),
        channel_id: trim_optional_string(value.channel_id),
        channel_name: trim_optional_string(value.channel_name),
        message_ts: trim_optional_string(value.message_ts)?,
        thread_ts: trim_optional_string(value.thread_ts),
        content: trim_optional_string(value.content).unwrap_or_default(),
        is_author_bot: value.is_author_bot.unwrap_or(false),
        permalink: trim_optional_string(value.permalink),
        context_messages: SlackMessageSearchContextMessages {
            before: value
                .context_messages
                .before
                .into_iter()
                .filter_map(map_context_message)
                .collect(),
            after: value
                .context_messages
                .after
                .into_iter()
                .filter_map(map_context_message)
                .collect(),
        },
    })
}

fn map_context_message(value: AssistantContextMessageDto) -> Option<SlackContextMessage> {
    Some(SlackContextMessage {
        author_name: trim_optional_string(value.author_name),
        user_id: trim_optional_string(value.user_id),
        ts: trim_optional_string(value.ts)?,
        text: trim_optional_string(value.text).unwrap_or_default(),
    })
}

fn trim_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_core::messaging::slack::{
        SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchSort,
        SlackMessageSearchSortDirection,
    };
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::SlackMessageSearchAdapter;
    use crate::outbound::slack::slack_web_api_client::{
        SlackWebApiClient, SlackWebApiClientConfig,
    };

    #[tokio::test]
    async fn posts_search_request_and_maps_message_results() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/assistant.search.context"))
            .and(header("Authorization", "Bearer xoxb-test"))
            .and(body_json(json!({
                "query": "error budget burn",
                "action_token": "action-token",
                "content_types": ["messages"],
                "channel_types": ["public_channel"],
                "include_bots": true,
                "include_context_messages": true,
                "limit": 5,
                "after": 1710000000,
                "sort": "timestamp",
                "sort_dir": "desc"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "results": {
                    "messages": [{
                        "author_name": "Jane Doe",
                        "author_user_id": "U123",
                        "team_id": "T123",
                        "channel_id": "C123",
                        "channel_name": "alerts-prod",
                        "message_ts": "1710000000.000001",
                        "thread_ts": "1710000000.000000",
                        "content": "Error budget burn is accelerating.",
                        "is_author_bot": false,
                        "permalink": "https://example.slack.com/archives/C123/p1710000000000001",
                        "context_messages": {
                            "before": [{
                                "author_name": "Alert Bot",
                                "user_id": "U999",
                                "ts": "1710000000.000000",
                                "text": "SLO alert fired"
                            }],
                            "after": [{
                                "author_name": "John Doe",
                                "user_id": "U124",
                                "ts": "1710000000.000002",
                                "text": "Looking into it"
                            }]
                        }
                    }]
                },
                "response_metadata": {
                    "next_cursor": "cursor-2"
                }
            })))
            .mount(&server)
            .await;

        let adapter = SlackMessageSearchAdapter::new(Arc::new(create_client(&server.uri())));
        let result = adapter
            .search_messages(SlackMessageSearchInput {
                query: " error budget burn ".to_string(),
                action_token: " action-token ".to_string(),
                limit: 5,
                include_bots: true,
                include_context_messages: true,
                before: None,
                after: Some(1_710_000_000),
                sort: SlackMessageSearchSort::Timestamp,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            })
            .await
            .expect("search slack messages");

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].author_name.as_deref(), Some("Jane Doe"));
        assert_eq!(
            result.messages[0].channel_name.as_deref(),
            Some("alerts-prod")
        );
        assert_eq!(result.messages[0].context_messages.before.len(), 1);
        assert_eq!(result.messages[0].context_messages.after.len(), 1);
        assert_eq!(result.next_cursor.as_deref(), Some("cursor-2"));
    }

    #[tokio::test]
    async fn rejects_invalid_limit_before_calling_slack() {
        let server = MockServer::start().await;
        let adapter = SlackMessageSearchAdapter::new(Arc::new(create_client(&server.uri())));

        let error = adapter
            .search_messages(SlackMessageSearchInput {
                query: "search".to_string(),
                action_token: "action-token".to_string(),
                limit: 6,
                include_bots: false,
                include_context_messages: true,
                before: None,
                after: None,
                sort: SlackMessageSearchSort::Score,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            })
            .await
            .expect_err("invalid input should fail");

        assert!(
            error
                .message
                .contains("Slack search limit must be between 1 and 5")
        );
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack client")
    }
}
