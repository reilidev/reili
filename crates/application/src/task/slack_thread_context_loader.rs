use std::sync::Arc;
use std::time::Instant;

use reili_core::messaging::slack::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage};

use super::logger::{TaskLogMeta, TaskLogger};
use crate::task::LogFieldValue;

#[derive(Debug, Clone, PartialEq)]
pub struct SlackThreadContextLoaderInput {
    pub message: SlackMessage,
    pub base_log_meta: TaskLogMeta,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadContextFetchFailedLogInput {
    pub base_log_meta: TaskLogMeta,
    pub thread_context_fetch_latency_ms: u128,
    pub error: String,
}

impl ThreadContextFetchFailedLogInput {
    fn into_log_meta(self) -> TaskLogMeta {
        let mut meta = self.base_log_meta;
        meta.insert(
            "thread_context_fetch_latency_ms".to_string(),
            LogFieldValue::from(self.thread_context_fetch_latency_ms),
        );
        meta.insert("error".to_string(), LogFieldValue::from(self.error));
        meta
    }
}

pub struct SlackThreadContextLoaderDeps {
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct SlackThreadContextLoader {
    deps: SlackThreadContextLoaderDeps,
}

impl SlackThreadContextLoader {
    pub fn new(deps: SlackThreadContextLoaderDeps) -> Self {
        Self { deps }
    }

    pub async fn load_for_message(
        &self,
        input: SlackThreadContextLoaderInput,
    ) -> Vec<SlackThreadMessage> {
        if !is_thread_reply_message(&input.message) {
            return Vec::new();
        }

        let thread_ts = input
            .message
            .thread_ts
            .clone()
            .unwrap_or_else(|| input.message.ts.clone());
        let started_at = Instant::now();
        match self
            .deps
            .slack_thread_history_port
            .fetch_thread_history(FetchSlackThreadHistoryInput {
                channel: input.message.channel,
                thread_ts,
            })
            .await
        {
            Ok(history) => history
                .into_iter()
                .filter(|message| !is_task_control_message(message))
                .collect(),
            Err(error) => {
                let log_input = ThreadContextFetchFailedLogInput {
                    base_log_meta: input.base_log_meta,
                    thread_context_fetch_latency_ms: started_at.elapsed().as_millis(),
                    error: error.message,
                };
                self.deps
                    .logger
                    .error("thread_context_fetch_failed", log_input.into_log_meta());
                Vec::new()
            }
        }
    }
}

fn is_thread_reply_message(message: &SlackMessage) -> bool {
    message
        .thread_ts
        .as_ref()
        .is_some_and(|thread_ts| thread_ts != &message.ts)
}

fn is_task_control_message(message: &SlackThreadMessage) -> bool {
    message
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.event_type == "task_control_message_posted")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::error::PortError;
    use reili_core::messaging::slack::{
        FetchSlackThreadHistoryInput, MockSlackThreadHistoryPort, SlackThreadHistoryPort,
    };
    use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage, SlackTriggerType};

    use crate::task::{LogFieldValue, TaskLogMeta, string_log_meta};

    use super::{
        SlackThreadContextLoader, SlackThreadContextLoaderDeps, SlackThreadContextLoaderInput,
    };

    #[derive(Debug, Clone, PartialEq)]
    struct LoggedError {
        message: String,
        meta: TaskLogMeta,
    }

    #[derive(Default)]
    struct ThreadContextLoaderLoggerMock {
        errors: Mutex<Vec<LoggedError>>,
    }

    impl ThreadContextLoaderLoggerMock {
        fn errors(&self) -> Vec<LoggedError> {
            self.errors.lock().expect("lock logger errors").clone()
        }
    }

    impl crate::task::TaskLogger for ThreadContextLoaderLoggerMock {
        fn log(&self, entry: crate::task::LogEntry) {
            self.errors
                .lock()
                .expect("lock logger errors")
                .push(LoggedError {
                    message: entry.event.to_string(),
                    meta: entry.fields,
                });
        }
    }

    #[tokio::test]
    async fn fetches_thread_history_only_for_thread_replies() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut thread_history_port = MockSlackThreadHistoryPort::new();
        let fetch_calls = Arc::clone(&calls);
        thread_history_port
            .expect_fetch_thread_history()
            .times(1)
            .returning(move |input: FetchSlackThreadHistoryInput| {
                fetch_calls.lock().expect("lock calls").push(input);
                Ok(vec![SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U001".to_string()),
                    text: "context".to_string(),
                    metadata: None,
                }])
            });
        let thread_history_port = Arc::new(thread_history_port);
        let logger = Arc::new(ThreadContextLoaderLoggerMock::default());
        let loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
            slack_thread_history_port: Arc::clone(&thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            logger,
        });

        let result = loader
            .load_for_message(SlackThreadContextLoaderInput {
                message: thread_reply_message(),
                base_log_meta: base_log_meta(),
            })
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").clone(),
            vec![FetchSlackThreadHistoryInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
            }]
        );
        assert_eq!(
            result,
            vec![SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U001".to_string()),
                text: "context".to_string(),
                metadata: None,
            }]
        );
    }

    #[tokio::test]
    async fn returns_empty_context_for_non_thread_messages() {
        let mut thread_history_port = MockSlackThreadHistoryPort::new();
        thread_history_port.expect_fetch_thread_history().times(0);
        let thread_history_port = Arc::new(thread_history_port);
        let logger = Arc::new(ThreadContextLoaderLoggerMock::default());
        let loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
            slack_thread_history_port: Arc::clone(&thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            logger,
        });

        let result = loader
            .load_for_message(SlackThreadContextLoaderInput {
                message: root_message(),
                base_log_meta: base_log_meta(),
            })
            .await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn falls_back_with_empty_context_when_history_fetch_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut thread_history_port = MockSlackThreadHistoryPort::new();
        let fetch_calls = Arc::clone(&calls);
        thread_history_port
            .expect_fetch_thread_history()
            .times(1)
            .returning(move |input: FetchSlackThreadHistoryInput| {
                fetch_calls.lock().expect("lock calls").push(input);
                Err(PortError::new("slack api failed"))
            });
        let thread_history_port = Arc::new(thread_history_port);
        let logger = Arc::new(ThreadContextLoaderLoggerMock::default());
        let loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
            slack_thread_history_port: Arc::clone(&thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            logger: Arc::clone(&logger) as Arc<dyn crate::task::TaskLogger>,
        });

        let result = loader
            .load_for_message(SlackThreadContextLoaderInput {
                message: thread_reply_message(),
                base_log_meta: base_log_meta(),
            })
            .await;

        assert!(result.is_empty());
        assert_eq!(
            calls.lock().expect("lock calls").clone(),
            vec![FetchSlackThreadHistoryInput {
                channel: "C001".to_string(),
                thread_ts: "1710000000.000001".to_string(),
            }]
        );
        let errors = logger.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "thread_context_fetch_failed");
        assert_eq!(
            errors[0].meta.get("jobId").and_then(LogFieldValue::as_str),
            Some("job-1")
        );
        assert_eq!(
            errors[0].meta.get("error").and_then(LogFieldValue::as_str),
            Some("slack api failed")
        );
    }

    fn thread_reply_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: None,
            action_token: None,
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "alert".to_string(),
            ts: "1710000000.000002".to_string(),
            thread_ts: Some("1710000000.000001".to_string()),
        }
    }

    fn root_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: None,
            action_token: None,
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "alert".to_string(),
            ts: "1710000000.000002".to_string(),
            thread_ts: None,
        }
    }

    fn base_log_meta() -> TaskLogMeta {
        string_log_meta([
            ("slackEventId", "Ev001"),
            ("jobId", "job-1"),
            ("channel", "C001"),
            ("threadTs", "1710000000.000001"),
            ("attempt", "1"),
        ])
    }
}
