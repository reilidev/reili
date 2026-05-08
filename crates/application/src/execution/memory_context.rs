use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use reili_core::messaging::slack::{
    SlackMessage, SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchResultItem,
    SlackMessageSearchSort, SlackMessageSearchSortDirection,
};
use reili_core::secret::SecretString;
use reili_core::task::{TaskMemoryItem, TaskMemorySource};

use crate::{LogFieldValue, TaskLogMeta, TaskLogger, string_log_meta};

const MEMORY_MARKER: &str = "reili_memory_v1";
const MEMORY_LIMIT: u32 = 10;
const MAX_MEMORY_CONTENT_CHARS: usize = 1_500;

#[derive(Debug, Clone, PartialEq)]
pub struct SlackMemoryContextLoaderInput {
    pub message: SlackMessage,
    pub started_at_unix_seconds: i64,
    pub base_log_meta: TaskLogMeta,
}

pub struct SlackMemoryContextLoaderDeps {
    pub slack_message_search_port: Arc<dyn SlackMessageSearchPort>,
    pub logger: Arc<dyn TaskLogger>,
    pub bot_user_id: String,
}

pub struct SlackMemoryContextLoader {
    deps: SlackMemoryContextLoaderDeps,
}

impl SlackMemoryContextLoader {
    pub fn new(deps: SlackMemoryContextLoaderDeps) -> Self {
        Self { deps }
    }

    pub async fn load_for_message(
        &self,
        input: SlackMemoryContextLoaderInput,
    ) -> Vec<TaskMemoryItem> {
        let Some(action_token) = input
            .message
            .action_token
            .as_ref()
            .map(SecretString::expose)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            self.deps.logger.info(
                "slack_memory_context_skipped",
                merge_log_meta(
                    &input.base_log_meta,
                    &string_log_meta([("reason", "missing_action_token")]),
                ),
            );
            return Vec::new();
        };

        let started_at = Instant::now();
        match self
            .deps
            .slack_message_search_port
            .search_messages(SlackMessageSearchInput {
                query: MEMORY_MARKER.to_string(),
                action_token: SecretString::from(action_token),
                context_channel_id: Some(input.message.channel.clone()),
                limit: MEMORY_LIMIT,
                include_bots: true,
                include_context_messages: false,
                before: Some(input.started_at_unix_seconds),
                after: None,
                sort: SlackMessageSearchSort::Timestamp,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            })
            .await
        {
            Ok(result) => build_memory_items(BuildMemoryItemsInput {
                messages: result.messages,
                current_channel_id: input.message.channel,
                bot_user_id: self.deps.bot_user_id.clone(),
            }),
            Err(error) => {
                let mut meta = input.base_log_meta;
                meta.insert(
                    "slack_memory_context_fetch_latency_ms".to_string(),
                    LogFieldValue::from(started_at.elapsed().as_millis()),
                );
                meta.insert("error".to_string(), LogFieldValue::from(error.message));
                self.deps
                    .logger
                    .warn("slack_memory_context_fetch_failed", meta);
                Vec::new()
            }
        }
    }
}

struct BuildMemoryItemsInput {
    messages: Vec<SlackMessageSearchResultItem>,
    current_channel_id: String,
    bot_user_id: String,
}

fn build_memory_items(input: BuildMemoryItemsInput) -> Vec<TaskMemoryItem> {
    let mut seen = HashSet::new();
    let mut items = input
        .messages
        .into_iter()
        .filter(|message| is_memory_message(message, &input.current_channel_id, &input.bot_user_id))
        .filter_map(|message| {
            let content = extract_memory_content(&message.content)?;
            let key = memory_dedupe_key(&message);
            if !seen.insert(key) {
                return None;
            }
            Some(TaskMemoryItem {
                source: TaskMemorySource {
                    channel_id: message.channel_id.unwrap_or_default(),
                    message_ts: message.message_ts,
                    thread_ts: message.thread_ts,
                    permalink: message.permalink,
                },
                content,
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| right.source.message_ts.cmp(&left.source.message_ts));
    items.truncate(MEMORY_LIMIT as usize);
    items
}

fn is_memory_message(
    message: &SlackMessageSearchResultItem,
    current_channel_id: &str,
    bot_user_id: &str,
) -> bool {
    message.is_author_bot
        && message.channel_id.as_deref() == Some(current_channel_id)
        && message.author_user_id.as_deref() == Some(bot_user_id)
        && message.content.contains(MEMORY_MARKER)
}

fn extract_memory_content(content: &str) -> Option<String> {
    let (_, memory_content) = content.split_once(MEMORY_MARKER)?;
    let memory_content = truncate_chars(memory_content.trim(), MAX_MEMORY_CONTENT_CHARS);
    if memory_content.is_empty() {
        None
    } else {
        Some(memory_content)
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut iter = value.char_indices();
    match iter.nth(max_chars) {
        Some((index, _)) => value[..index].to_string(),
        None => value.to_string(),
    }
}

fn memory_dedupe_key(message: &SlackMessageSearchResultItem) -> String {
    message.permalink.clone().unwrap_or_else(|| {
        format!(
            "{}:{}",
            message.channel_id.as_deref().unwrap_or(""),
            message.message_ts
        )
    })
}

fn merge_log_meta(base: &TaskLogMeta, append: &TaskLogMeta) -> TaskLogMeta {
    let mut merged = base.clone();
    merged.extend(append.clone());
    merged
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::logger::LogEntry;
    use reili_core::messaging::slack::{
        MockSlackMessageSearchPort, SlackMessage, SlackMessageSearchContextMessages,
        SlackMessageSearchInput, SlackMessageSearchPort, SlackMessageSearchResult,
        SlackMessageSearchResultItem, SlackMessageSearchSort, SlackMessageSearchSortDirection,
        SlackTriggerType,
    };
    use reili_core::secret::SecretString;

    use crate::{TaskLogger, string_log_meta};

    use super::{
        SlackMemoryContextLoader, SlackMemoryContextLoaderDeps, SlackMemoryContextLoaderInput,
    };

    #[derive(Default)]
    struct MemoryLoaderLoggerMock {
        logs: Mutex<Vec<LogEntry>>,
    }

    impl MemoryLoaderLoggerMock {
        fn logs(&self) -> Vec<LogEntry> {
            self.logs.lock().expect("lock logs").clone()
        }
    }

    impl TaskLogger for MemoryLoaderLoggerMock {
        fn log(&self, entry: LogEntry) {
            self.logs.lock().expect("lock logs").push(entry);
        }
    }

    #[tokio::test]
    async fn searches_slack_memory_and_filters_reili_marker_messages() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let search_calls = Arc::clone(&calls);
        let mut slack_message_search_port = MockSlackMessageSearchPort::new();
        slack_message_search_port
            .expect_search_messages()
            .times(1)
            .returning(move |input: SlackMessageSearchInput| {
                search_calls.lock().expect("lock calls").push(input);
                Ok(SlackMessageSearchResult {
                    messages: vec![
                        memory_message("1760000000.000002", Some("https://slack/memory-2")),
                        user_message("1760000000.000003"),
                        other_channel_message("1760000000.000004"),
                        memory_message("1760000000.000001", Some("https://slack/memory-1")),
                    ],
                    next_cursor: None,
                })
            });
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            slack_message_search_port: Arc::new(slack_message_search_port)
                as Arc<dyn SlackMessageSearchPort>,
            logger,
            bot_user_id: "UBOT".to_string(),
        });

        let result = loader
            .load_for_message(SlackMemoryContextLoaderInput {
                message: trigger_message(Some("action-token")),
                started_at_unix_seconds: 1_760_000_100,
                base_log_meta: string_log_meta([("jobId", "job-1")]),
            })
            .await;

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].source.message_ts, "1760000000.000002");
        assert_eq!(result[0].content, "- service: checkout-api");
        assert_eq!(result[1].source.message_ts, "1760000000.000001");

        let captured = calls.lock().expect("lock calls").clone();
        assert_eq!(
            captured,
            vec![SlackMessageSearchInput {
                query: "reili_memory_v1".to_string(),
                action_token: SecretString::from("action-token"),
                context_channel_id: Some("C001".to_string()),
                limit: 10,
                include_bots: true,
                include_context_messages: false,
                before: Some(1_760_000_100),
                after: None,
                sort: SlackMessageSearchSort::Timestamp,
                sort_direction: SlackMessageSearchSortDirection::Desc,
            }]
        );
    }

    #[tokio::test]
    async fn returns_empty_when_action_token_is_missing() {
        let mut slack_message_search_port = MockSlackMessageSearchPort::new();
        slack_message_search_port.expect_search_messages().times(0);
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            slack_message_search_port: Arc::new(slack_message_search_port)
                as Arc<dyn SlackMessageSearchPort>,
            logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
            bot_user_id: "UBOT".to_string(),
        });

        let result = loader
            .load_for_message(SlackMemoryContextLoaderInput {
                message: trigger_message(None),
                started_at_unix_seconds: 1_760_000_100,
                base_log_meta: string_log_meta([("jobId", "job-1")]),
            })
            .await;

        assert!(result.is_empty());
        assert_eq!(logger.logs().len(), 1);
    }

    #[tokio::test]
    async fn returns_empty_when_slack_search_fails() {
        let mut slack_message_search_port = MockSlackMessageSearchPort::new();
        slack_message_search_port
            .expect_search_messages()
            .times(1)
            .returning(|_| Err(PortError::new("slack failed")));
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            slack_message_search_port: Arc::new(slack_message_search_port)
                as Arc<dyn SlackMessageSearchPort>,
            logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
            bot_user_id: "UBOT".to_string(),
        });

        let result = loader
            .load_for_message(SlackMemoryContextLoaderInput {
                message: trigger_message(Some("action-token")),
                started_at_unix_seconds: 1_760_000_100,
                base_log_meta: string_log_meta([("jobId", "job-1")]),
            })
            .await;

        assert!(result.is_empty());
        assert_eq!(logger.logs()[0].event, "slack_memory_context_fetch_failed");
    }

    #[tokio::test]
    async fn deduplicates_by_permalink() {
        let mut slack_message_search_port = MockSlackMessageSearchPort::new();
        slack_message_search_port
            .expect_search_messages()
            .times(1)
            .returning(|_| {
                Ok(SlackMessageSearchResult {
                    messages: vec![
                        memory_message("1760000000.000002", Some("https://slack/same")),
                        memory_message("1760000000.000001", Some("https://slack/same")),
                    ],
                    next_cursor: None,
                })
            });
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            slack_message_search_port: Arc::new(slack_message_search_port)
                as Arc<dyn SlackMessageSearchPort>,
            logger,
            bot_user_id: "UBOT".to_string(),
        });

        let result = loader
            .load_for_message(SlackMemoryContextLoaderInput {
                message: trigger_message(Some("action-token")),
                started_at_unix_seconds: 1_760_000_100,
                base_log_meta: string_log_meta([("jobId", "job-1")]),
            })
            .await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source.message_ts, "1760000000.000002");
    }

    fn trigger_message(action_token: Option<&str>) -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: action_token.map(SecretString::from),
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "<@UBOT> investigate".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1760000100.000001".to_string(),
            thread_ts: None,
        }
    }

    fn memory_message(message_ts: &str, permalink: Option<&str>) -> SlackMessageSearchResultItem {
        SlackMessageSearchResultItem {
            author_name: Some("Reili".to_string()),
            author_user_id: Some("UBOT".to_string()),
            team_id: Some("T001".to_string()),
            channel_id: Some("C001".to_string()),
            channel_name: Some("alerts".to_string()),
            message_ts: message_ts.to_string(),
            thread_ts: Some("1760000000.000000".to_string()),
            content: "*Reusable notes*\nreili_memory_v1\n- service: checkout-api".to_string(),
            is_author_bot: true,
            permalink: permalink.map(ToString::to_string),
            context_messages: SlackMessageSearchContextMessages {
                before: Vec::new(),
                after: Vec::new(),
            },
        }
    }

    fn user_message(message_ts: &str) -> SlackMessageSearchResultItem {
        SlackMessageSearchResultItem {
            author_user_id: Some("UUSER".to_string()),
            is_author_bot: false,
            ..memory_message(message_ts, Some("https://slack/user"))
        }
    }

    fn other_channel_message(message_ts: &str) -> SlackMessageSearchResultItem {
        SlackMessageSearchResultItem {
            channel_id: Some("C999".to_string()),
            ..memory_message(message_ts, Some("https://slack/other-channel"))
        }
    }
}
