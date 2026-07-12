use std::sync::Arc;
use std::time::Instant;

use reili_core::messaging::slack::{
    ListSlackCanvasMemoriesInput, SlackCanvasMemoryPort, SlackCanvasMemoryRecord,
    SlackCanvasMemoryVisibility,
};
use reili_core::task::TaskMemoryItem;

use crate::{LogFieldValue, TaskLogMeta, TaskLogger, string_log_meta};

const MEMORY_LIMIT: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct SlackMemoryContextLoaderInput {
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub base_log_meta: TaskLogMeta,
}

pub struct SlackMemoryContextLoaderDeps {
    /// `None` disables the memory feature; the loader yields an empty context.
    pub canvas_memory_port: Option<Arc<dyn SlackCanvasMemoryPort>>,
    pub logger: Arc<dyn TaskLogger>,
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
        let Some(canvas_memory_port) = self.deps.canvas_memory_port.as_ref() else {
            self.deps.logger.info(
                "slack_memory_context_skipped",
                merge_log_meta(
                    &input.base_log_meta,
                    &string_log_meta([("reason", "memory_disabled")]),
                ),
            );
            return Vec::new();
        };

        let started_at = Instant::now();
        match canvas_memory_port
            .list_channel_memories(ListSlackCanvasMemoriesInput {
                channel_id: input.channel_id,
                channel_name: input.channel_name,
                limit: MEMORY_LIMIT,
            })
            .await
        {
            // The port already caps each group (shared + channel) at `limit`; keep both groups.
            Ok(records) => records.into_iter().map(record_to_item).collect(),
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

fn record_to_item(record: SlackCanvasMemoryRecord) -> TaskMemoryItem {
    TaskMemoryItem {
        fact: record.fact,
        evidence: record.evidence,
        scope: record.scope,
        source_url: record.source_url,
        created_at: record.created_at,
        shared: matches!(record.visibility, SlackCanvasMemoryVisibility::Shared),
    }
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
        ListSlackCanvasMemoriesInput, MockSlackCanvasMemoryPort, SlackCanvasMemoryPort,
        SlackCanvasMemoryRecord, SlackCanvasMemoryVisibility,
    };

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

    fn record(created_at: &str, fact: &str) -> SlackCanvasMemoryRecord {
        record_with_visibility(created_at, fact, SlackCanvasMemoryVisibility::Channel)
    }

    fn record_with_visibility(
        created_at: &str,
        fact: &str,
        visibility: SlackCanvasMemoryVisibility,
    ) -> SlackCanvasMemoryRecord {
        SlackCanvasMemoryRecord {
            visibility,
            fact: fact.to_string(),
            evidence: "evidence".to_string(),
            scope: "scope".to_string(),
            source_url: Some("https://slack/memory".to_string()),
            created_at: created_at.to_string(),
        }
    }

    fn loader_input() -> SlackMemoryContextLoaderInput {
        SlackMemoryContextLoaderInput {
            channel_id: "C001".to_string(),
            channel_name: Some("alerts".to_string()),
            base_log_meta: string_log_meta([("jobId", "job-1")]),
        }
    }

    #[tokio::test]
    async fn maps_canvas_records_to_memory_items() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let list_calls = Arc::clone(&calls);
        let mut canvas_memory_port = MockSlackCanvasMemoryPort::new();
        canvas_memory_port
            .expect_list_channel_memories()
            .times(1)
            .returning(move |input: ListSlackCanvasMemoriesInput| {
                list_calls.lock().expect("lock calls").push(input);
                Ok(vec![
                    record("2026-07-07T09:12:34Z", "newer"),
                    record("2026-07-06T22:40:11Z", "older"),
                ])
            });
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            canvas_memory_port: Some(Arc::new(canvas_memory_port) as Arc<dyn SlackCanvasMemoryPort>),
            logger,
        });

        let result = loader.load_for_message(loader_input()).await;

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].fact, "newer");
        assert_eq!(result[0].created_at, "2026-07-07T09:12:34Z");
        assert!(!result[0].shared);
        assert_eq!(result[1].fact, "older");

        let captured = calls.lock().expect("lock calls").clone();
        assert_eq!(
            captured,
            vec![ListSlackCanvasMemoriesInput {
                channel_id: "C001".to_string(),
                channel_name: Some("alerts".to_string()),
                limit: 10,
            }]
        );
    }

    #[tokio::test]
    async fn maps_shared_visibility_to_shared_flag() {
        let mut canvas_memory_port = MockSlackCanvasMemoryPort::new();
        canvas_memory_port
            .expect_list_channel_memories()
            .times(1)
            .returning(|_| {
                Ok(vec![record_with_visibility(
                    "2026-07-08T10:00:00Z",
                    "shared fact",
                    SlackCanvasMemoryVisibility::Shared,
                )])
            });
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            canvas_memory_port: Some(Arc::new(canvas_memory_port) as Arc<dyn SlackCanvasMemoryPort>),
            logger,
        });

        let result = loader.load_for_message(loader_input()).await;

        assert_eq!(result.len(), 1);
        assert!(result[0].shared);
    }

    #[tokio::test]
    async fn returns_empty_when_memory_is_disabled() {
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            canvas_memory_port: None,
            logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
        });

        let result = loader.load_for_message(loader_input()).await;

        assert!(result.is_empty());
        assert_eq!(logger.logs().len(), 1);
        assert_eq!(logger.logs()[0].event, "slack_memory_context_skipped");
    }

    #[tokio::test]
    async fn returns_empty_when_canvas_fetch_fails() {
        let mut canvas_memory_port = MockSlackCanvasMemoryPort::new();
        canvas_memory_port
            .expect_list_channel_memories()
            .times(1)
            .returning(|_| Err(PortError::new("canvas failed")));
        let logger = Arc::new(MemoryLoaderLoggerMock::default());
        let loader = SlackMemoryContextLoader::new(SlackMemoryContextLoaderDeps {
            canvas_memory_port: Some(Arc::new(canvas_memory_port) as Arc<dyn SlackCanvasMemoryPort>),
            logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
        });

        let result = loader.load_for_message(loader_input()).await;

        assert!(result.is_empty());
        assert_eq!(logger.logs()[0].event, "slack_memory_context_fetch_failed");
    }
}
