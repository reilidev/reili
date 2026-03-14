use std::sync::Arc;
use std::time::Instant;

use reili_core::messaging::slack::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage};

use super::logger::InvestigationLogMeta;

#[derive(Debug, Clone, PartialEq)]
pub struct SlackThreadContextLoaderInput {
    pub message: SlackMessage,
    pub base_log_meta: InvestigationLogMeta,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadContextFetchFailedLogInput {
    pub base_log_meta: InvestigationLogMeta,
    pub thread_context_fetch_latency_ms: u128,
    pub error: String,
}

pub trait ThreadContextLoaderLogger: Send + Sync {
    fn error(&self, message: &str, input: ThreadContextFetchFailedLogInput);
}

pub struct SlackThreadContextLoaderDeps {
    pub slack_thread_history_port: Arc<dyn SlackThreadHistoryPort>,
    pub logger: Arc<dyn ThreadContextLoaderLogger>,
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
            Ok(history) => history,
            Err(error) => {
                self.deps.logger.error(
                    "thread_context_fetch_failed",
                    ThreadContextFetchFailedLogInput {
                        base_log_meta: input.base_log_meta,
                        thread_context_fetch_latency_ms: started_at.elapsed().as_millis(),
                        error: error.message,
                    },
                );
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::messaging::slack::{FetchSlackThreadHistoryInput, SlackThreadHistoryPort};
    use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage, SlackTriggerType};
    use serde_json::Value;

    use crate::investigation::InvestigationLogMeta;

    use super::{
        SlackThreadContextLoader, SlackThreadContextLoaderDeps, SlackThreadContextLoaderInput,
        ThreadContextFetchFailedLogInput, ThreadContextLoaderLogger,
    };

    #[derive(Debug, Clone, PartialEq)]
    struct LoggedError {
        message: String,
        input: ThreadContextFetchFailedLogInput,
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

    impl ThreadContextLoaderLogger for ThreadContextLoaderLoggerMock {
        fn error(&self, message: &str, input: ThreadContextFetchFailedLogInput) {
            self.errors
                .lock()
                .expect("lock logger errors")
                .push(LoggedError {
                    message: message.to_string(),
                    input,
                });
        }
    }

    struct SlackThreadHistoryPortMock {
        calls: Mutex<Vec<FetchSlackThreadHistoryInput>>,
        response: Result<Vec<SlackThreadMessage>, PortError>,
    }

    impl SlackThreadHistoryPortMock {
        fn success(messages: Vec<SlackThreadMessage>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Ok(messages),
            }
        }

        fn failure(message: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Err(PortError::new(message)),
            }
        }

        fn calls(&self) -> Vec<FetchSlackThreadHistoryInput> {
            self.calls.lock().expect("lock calls").clone()
        }
    }

    #[async_trait]
    impl SlackThreadHistoryPort for SlackThreadHistoryPortMock {
        async fn fetch_thread_history(
            &self,
            input: FetchSlackThreadHistoryInput,
        ) -> Result<Vec<SlackThreadMessage>, PortError> {
            self.calls.lock().expect("lock calls").push(input);
            self.response.clone()
        }
    }

    #[tokio::test]
    async fn fetches_thread_history_only_for_thread_replies() {
        let thread_history_port = Arc::new(SlackThreadHistoryPortMock::success(vec![
            SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U001".to_string()),
                text: "context".to_string(),
            },
        ]));
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
            thread_history_port.calls(),
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
            }]
        );
    }

    #[tokio::test]
    async fn returns_empty_context_for_non_thread_messages() {
        let thread_history_port = Arc::new(SlackThreadHistoryPortMock::success(Vec::new()));
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

        assert!(thread_history_port.calls().is_empty());
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn falls_back_with_empty_context_when_history_fetch_fails() {
        let thread_history_port = Arc::new(SlackThreadHistoryPortMock::failure("slack api failed"));
        let logger = Arc::new(ThreadContextLoaderLoggerMock::default());
        let loader = SlackThreadContextLoader::new(SlackThreadContextLoaderDeps {
            slack_thread_history_port: Arc::clone(&thread_history_port)
                as Arc<dyn SlackThreadHistoryPort>,
            logger: Arc::clone(&logger) as Arc<dyn ThreadContextLoaderLogger>,
        });

        let result = loader
            .load_for_message(SlackThreadContextLoaderInput {
                message: thread_reply_message(),
                base_log_meta: base_log_meta(),
            })
            .await;

        assert!(result.is_empty());
        let errors = logger.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "thread_context_fetch_failed");
        assert_eq!(
            errors[0]
                .input
                .base_log_meta
                .get("jobId")
                .and_then(Value::as_str),
            Some("job-1")
        );
    }

    fn thread_reply_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: None,
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
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "alert".to_string(),
            ts: "1710000000.000002".to_string(),
            thread_ts: None,
        }
    }

    fn base_log_meta() -> InvestigationLogMeta {
        serde_json::Map::from_iter([
            (
                "slackEventId".to_string(),
                Value::String("Ev001".to_string()),
            ),
            ("jobId".to_string(), Value::String("job-1".to_string())),
            ("channel".to_string(), Value::String("C001".to_string())),
            (
                "threadTs".to_string(),
                Value::String("1710000000.000001".to_string()),
            ),
            ("attempt".to_string(), Value::String("1".to_string())),
        ])
    }
}
