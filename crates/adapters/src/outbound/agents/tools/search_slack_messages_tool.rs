use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::messaging::slack::{
    SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchSort,
    SlackMessageSearchSortDirection,
};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::slack_tool_soft_error::{
    build_capability_unavailable_soft_error, to_slack_tool_soft_error,
};
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchSlackMessagesTool {
    slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
    action_token: Option<String>,
}

impl SearchSlackMessagesTool {
    pub fn new(
        slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
        action_token: Option<String>,
    ) -> Self {
        Self {
            slack_message_search_port,
            action_token,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSlackMessagesArgs {
    pub query: String,
    #[serde(default = "default_search_slack_limit")]
    pub limit: u32,
    #[serde(default = "default_include_bots")]
    pub include_bots: bool,
    #[serde(default = "default_include_context_messages")]
    pub include_context_messages: bool,
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub after: Option<i64>,
    #[serde(default = "default_search_slack_sort")]
    pub sort: SlackMessageSearchSort,
    #[serde(default = "default_search_slack_sort_direction")]
    pub sort_direction: SlackMessageSearchSortDirection,
}

fn default_search_slack_limit() -> u32 {
    5
}

fn default_include_bots() -> bool {
    true
}

fn default_include_context_messages() -> bool {
    true
}

fn default_search_slack_sort() -> SlackMessageSearchSort {
    SlackMessageSearchSort::Score
}

fn default_search_slack_sort_direction() -> SlackMessageSearchSortDirection {
    SlackMessageSearchSortDirection::Desc
}

impl Tool for SearchSlackMessagesTool {
    const NAME: &'static str = "search_slack_messages";

    type Error = PortError;
    type Args = SearchSlackMessagesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search prior Slack messages in the current Slack invocation context. Results are limited by Slack permissions, the originating conversation context, and the app's bot-token search scopes.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Slack search query. Prefer concise plain text or valid Slack search filters.",
                        "maxLength": 500
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5,
                        "default": 5,
                        "description": "Maximum number of message results to return."
                    },
                    "includeBots": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to include bot-authored messages."
                    },
                    "includeContextMessages": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to include surrounding before/after messages for each hit."
                    },
                    "before": {
                        "type": "integer",
                        "description": "Optional upper bound as a UNIX timestamp in seconds."
                    },
                    "after": {
                        "type": "integer",
                        "description": "Optional lower bound as a UNIX timestamp in seconds."
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["score", "timestamp"],
                        "default": "score",
                        "description": "Sort by relevance score or timestamp."
                    },
                    "sortDirection": {
                        "type": "string",
                        "enum": ["asc", "desc"],
                        "default": "desc",
                        "description": "Sort direction."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let Some(action_token) = self
            .action_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return to_json_string(&build_capability_unavailable_soft_error(
                "Slack search is unavailable because the triggering Slack event did not include an action_token.",
            ));
        };

        match self
            .slack_message_search_port
            .search_messages(SlackMessageSearchInput {
                query: args.query,
                action_token: action_token.to_string(),
                limit: args.limit,
                include_bots: args.include_bots,
                include_context_messages: args.include_context_messages,
                before: args.before,
                after: args.after,
                sort: args.sort,
                sort_direction: args.sort_direction,
            })
            .await
        {
            Ok(result) => to_json_string(&result),
            Err(error) => to_json_string(&to_slack_tool_soft_error(&error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::messaging::slack::{
        SlackMessageSearchContextMessages, SlackMessageSearchInput, SlackMessageSearchPort,
        SlackMessageSearchResult, SlackMessageSearchResultItem, SlackMessageSearchSort,
        SlackMessageSearchSortDirection,
    };
    use rig::tool::Tool;

    use super::{SearchSlackMessagesArgs, SearchSlackMessagesTool};

    struct MockSlackSearchPort {
        calls: Arc<Mutex<Vec<SlackMessageSearchInput>>>,
        result: SlackMessageSearchResult,
    }

    #[async_trait]
    impl SlackMessageSearchPort for MockSlackSearchPort {
        async fn search_messages(
            &self,
            input: SlackMessageSearchInput,
        ) -> Result<SlackMessageSearchResult, PortError> {
            self.calls.lock().expect("lock calls").push(input);
            Ok(self.result.clone())
        }
    }

    fn build_tool(
        calls: Arc<Mutex<Vec<SlackMessageSearchInput>>>,
        action_token: Option<String>,
        result: SlackMessageSearchResult,
    ) -> SearchSlackMessagesTool {
        SearchSlackMessagesTool::new(
            Arc::new(MockSlackSearchPort { calls, result }),
            action_token,
        )
    }

    #[tokio::test]
    async fn converts_args_to_search_input_and_returns_json() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            Some("action-token".to_string()),
            SlackMessageSearchResult {
                messages: vec![SlackMessageSearchResultItem {
                    author_name: Some("Jane Doe".to_string()),
                    author_user_id: Some("U123".to_string()),
                    team_id: Some("T123".to_string()),
                    channel_id: Some("C123".to_string()),
                    channel_name: Some("alerts".to_string()),
                    message_ts: "1710000000.000001".to_string(),
                    thread_ts: None,
                    content: "Discussing rollout issue".to_string(),
                    is_author_bot: false,
                    permalink: Some(
                        "https://example.slack.com/archives/C123/p1710000000000001".to_string(),
                    ),
                    context_messages: SlackMessageSearchContextMessages {
                        before: Vec::new(),
                        after: Vec::new(),
                    },
                }],
                next_cursor: None,
            },
        );

        let output = tool
            .call(SearchSlackMessagesArgs {
                query: "rollout issue".to_string(),
                limit: 5,
                include_bots: true,
                include_context_messages: true,
                before: None,
                after: Some(1_710_000_000),
                sort: SlackMessageSearchSort::Timestamp,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            })
            .await
            .expect("call search_slack_messages");

        assert!(output.contains("Discussing rollout issue"));

        let captured = calls.lock().expect("lock calls");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].query, "rollout issue");
        assert_eq!(captured[0].action_token, "action-token");
        assert_eq!(captured[0].limit, 5);
    }

    #[tokio::test]
    async fn returns_soft_error_when_action_token_is_missing() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            Arc::clone(&calls),
            None,
            SlackMessageSearchResult {
                messages: Vec::new(),
                next_cursor: None,
            },
        );

        let output = tool
            .call(SearchSlackMessagesArgs {
                query: "outage".to_string(),
                limit: 5,
                include_bots: true,
                include_context_messages: true,
                before: None,
                after: None,
                sort: SlackMessageSearchSort::Score,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            })
            .await
            .expect("call search_slack_messages");

        assert!(output.contains("capability_unavailable"));
        assert!(calls.lock().expect("lock calls").is_empty());
    }

    #[tokio::test]
    async fn tool_schema_has_required_query() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tool = build_tool(
            calls,
            Some("action-token".to_string()),
            SlackMessageSearchResult {
                messages: Vec::new(),
                next_cursor: None,
            },
        );

        let definition = tool.definition("test".to_string()).await;
        assert_eq!(definition.name, "search_slack_messages");
        assert_eq!(definition.parameters["properties"]["limit"]["default"], 5);
        assert_eq!(definition.parameters["properties"]["limit"]["maximum"], 5);
        let required = definition.parameters["required"]
            .as_array()
            .expect("required array");
        assert!(required.contains(&serde_json::Value::String("query".to_string())));
    }
}
