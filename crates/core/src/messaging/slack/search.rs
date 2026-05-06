use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::secret::SecretString;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackMessageSearchSort {
    Score,
    Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlackMessageSearchSortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessageSearchInput {
    pub query: String,
    pub action_token: SecretString,
    pub limit: u32,
    pub include_bots: bool,
    pub include_context_messages: bool,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub sort: SlackMessageSearchSort,
    pub sort_direction: SlackMessageSearchSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessageSearchResult {
    pub messages: Vec<SlackMessageSearchResultItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessageSearchResultItem {
    pub author_name: Option<String>,
    pub author_user_id: Option<String>,
    pub team_id: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub message_ts: String,
    pub thread_ts: Option<String>,
    pub content: String,
    pub is_author_bot: bool,
    pub permalink: Option<String>,
    pub context_messages: SlackMessageSearchContextMessages,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessageSearchContextMessages {
    pub before: Vec<SlackContextMessage>,
    pub after: Vec<SlackContextMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackContextMessage {
    pub author_name: Option<String>,
    pub user_id: Option<String>,
    pub ts: String,
    pub text: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackMessageSearchPort: Send + Sync {
    async fn search_messages(
        &self,
        input: SlackMessageSearchInput,
    ) -> Result<SlackMessageSearchResult, PortError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        SlackContextMessage, SlackMessageSearchContextMessages, SlackMessageSearchResult,
        SlackMessageSearchResultItem,
    };

    #[test]
    fn serializes_result_with_context_messages() {
        let result = SlackMessageSearchResult {
            messages: vec![SlackMessageSearchResultItem {
                author_name: Some("Jane Doe".to_string()),
                author_user_id: Some("U123".to_string()),
                team_id: Some("T123".to_string()),
                channel_id: Some("C123".to_string()),
                channel_name: Some("alerts".to_string()),
                message_ts: "1710000000.000001".to_string(),
                thread_ts: Some("1710000000.000000".to_string()),
                content: "investigation started".to_string(),
                is_author_bot: false,
                permalink: Some(
                    "https://example.slack.com/archives/C123/p1710000000000001".to_string(),
                ),
                context_messages: SlackMessageSearchContextMessages {
                    before: vec![SlackContextMessage {
                        author_name: Some("Bot".to_string()),
                        user_id: Some("U999".to_string()),
                        ts: "1710000000.000000".to_string(),
                        text: "alert fired".to_string(),
                    }],
                    after: Vec::new(),
                },
            }],
            next_cursor: Some("cursor-1".to_string()),
        };

        let value = serde_json::to_value(&result).expect("serialize result");

        assert_eq!(
            value,
            json!({
                "messages": [{
                    "author_name": "Jane Doe",
                    "author_user_id": "U123",
                    "team_id": "T123",
                    "channel_id": "C123",
                    "channel_name": "alerts",
                    "message_ts": "1710000000.000001",
                    "thread_ts": "1710000000.000000",
                    "content": "investigation started",
                    "is_author_bot": false,
                    "permalink": "https://example.slack.com/archives/C123/p1710000000000001",
                    "context_messages": {
                        "before": [{
                            "author_name": "Bot",
                            "user_id": "U999",
                            "ts": "1710000000.000000",
                            "text": "alert fired"
                        }],
                        "after": []
                    }
                }],
                "next_cursor": "cursor-1"
            })
        );
    }
}
