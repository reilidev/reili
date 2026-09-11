use std::sync::Arc;

use reili_core::logger::{LogFieldValue, Logger, log_fields};
use reili_core::task::{
    TaskCancellation, TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort, TaskRuntime,
};
use rig::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, CompletionResponseEvent, HookContext,
    ObservationAction, StreamResponseFinish, TextDelta, ToolCall, ToolCallAction, ToolCallDelta,
    ToolResultAction, ToolResultEvent,
};
use rig::tool::ToolResult;

use super::usage_collector::LlmUsageCollector;

const REPORT_PROGRESS_TOOL_NAME: &str = "report_progress";
const TASK_CANCELLED_REASON: &str = "task_cancelled";
const NATIVE_WEB_SEARCH_TOOL_NAME: &str = "web_search";

#[derive(Clone)]
pub struct AgentExecutionHook {
    owner_id: String,
    runtime: TaskRuntime,
    cancellation: TaskCancellation,
    logger: Arc<dyn Logger>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    usage_collector: LlmUsageCollector,
}

impl AgentExecutionHook {
    pub fn new(
        owner_id: String,
        runtime: TaskRuntime,
        cancellation: TaskCancellation,
        logger: Arc<dyn Logger>,
        on_progress_event: Arc<dyn TaskProgressEventPort>,
        usage_collector: LlmUsageCollector,
    ) -> Self {
        Self {
            owner_id,
            runtime,
            cancellation,
            logger,
            on_progress_event,
            usage_collector,
        }
    }

    async fn publish_tool_started(&self, tool_name: &str, task_id: &str) {
        if tool_name == REPORT_PROGRESS_TOOL_NAME {
            return;
        }

        let publish_result = self
            .on_progress_event
            .publish(TaskProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: TaskProgressEvent::ToolCallStarted {
                    task_id: task_id.to_string(),
                    title: tool_name.to_string(),
                },
            })
            .await;
        if let Err(error) = publish_result {
            tracing::error!(
                owner_id = self.owner_id,
                tool_name,
                task_id,
                error = error.message,
                "Failed to publish tool started progress event",
            );
        }
    }

    async fn publish_tool_completed(&self, tool_name: &str, task_id: &str) {
        if tool_name == REPORT_PROGRESS_TOOL_NAME {
            return;
        }

        let publish_result = self
            .on_progress_event
            .publish(TaskProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: TaskProgressEvent::ToolCallCompleted {
                    task_id: task_id.to_string(),
                    title: tool_name.to_string(),
                },
            })
            .await;
        if let Err(error) = publish_result {
            tracing::error!(
                owner_id = self.owner_id,
                tool_name,
                task_id,
                error = error.message,
                "Failed to publish tool completed progress event",
            );
        }
    }

    fn log_tool_started(&self, tool_name: &str, task_id: &str) {
        self.logger.info(
            "llm_tool_execution_started",
            log_fields([
                ("ownerId", LogFieldValue::from(self.owner_id.clone())),
                ("toolName", LogFieldValue::from(tool_name.to_string())),
                ("toolCallId", LogFieldValue::from(task_id.to_string())),
                ("channel", LogFieldValue::from(self.runtime.channel.clone())),
                (
                    "threadTs",
                    LogFieldValue::from(self.runtime.thread_ts.clone()),
                ),
                ("retryCount", LogFieldValue::from(self.runtime.retry_count)),
            ]),
        );
    }

    fn log_tool_completed(&self, tool_name: &str, task_id: &str, raw_result: &ToolResult) {
        self.logger.info(
            "llm_tool_execution_completed",
            log_fields([
                ("ownerId", LogFieldValue::from(self.owner_id.clone())),
                ("toolName", LogFieldValue::from(tool_name.to_string())),
                ("toolCallId", LogFieldValue::from(task_id.to_string())),
                ("channel", LogFieldValue::from(self.runtime.channel.clone())),
                (
                    "threadTs",
                    LogFieldValue::from(self.runtime.thread_ts.clone()),
                ),
                ("retryCount", LogFieldValue::from(self.runtime.retry_count)),
                (
                    "result",
                    LogFieldValue::from(classify_tool_result(raw_result).to_string()),
                ),
            ]),
        );
    }

    fn track_completion_call(&self) {
        self.usage_collector.record_request();
    }

    fn track_completion_response(&self, usage: rig::completion::Usage) {
        self.usage_collector.record_usage(&usage);
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    async fn handle_completion_call(&self) -> CompletionCallAction {
        if self.is_cancelled() {
            return CompletionCallAction::stop(TASK_CANCELLED_REASON);
        }
        self.track_completion_call();
        CompletionCallAction::continue_run()
    }

    async fn handle_completion_response(
        &self,
        usage: rig::completion::Usage,
        raw: &serde_json::Value,
    ) -> ObservationAction {
        self.track_completion_response(usage);
        self.handle_native_web_search_calls(raw).await;
        if self.is_cancelled() {
            return ObservationAction::stop(TASK_CANCELLED_REASON);
        }
        ObservationAction::continue_run()
    }

    /// Bedrock Mantle's native `web_search` tool (see `build_provider_settings` in
    /// `bedrock_mantle.rs`) runs server-side, so it never reaches
    /// `on_tool_call`/`on_tool_result` the way the local `search_web` tool does — this is its
    /// only observation point. Matches on the raw `type` string rather than deserializing into a
    /// provider-specific response type, so it's a silent no-op for every other provider's
    /// response shape.
    async fn handle_native_web_search_calls(&self, raw: &serde_json::Value) {
        let Some(items) = raw.get("output").and_then(serde_json::Value::as_array) else {
            return;
        };

        for item in items {
            if item.get("type").and_then(serde_json::Value::as_str) != Some("web_search_call") {
                continue;
            }

            let tool_call_id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");

            self.logger.info(
                "llm_native_web_search_call_observed",
                log_fields([
                    ("ownerId", LogFieldValue::from(self.owner_id.clone())),
                    ("toolCallId", LogFieldValue::from(tool_call_id.to_string())),
                    ("channel", LogFieldValue::from(self.runtime.channel.clone())),
                    (
                        "threadTs",
                        LogFieldValue::from(self.runtime.thread_ts.clone()),
                    ),
                    ("retryCount", LogFieldValue::from(self.runtime.retry_count)),
                    (
                        "status",
                        LogFieldValue::from(
                            item.get("status")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("unknown")
                                .to_string(),
                        ),
                    ),
                ]),
            );

            // The search already finished, so this fires back to back rather than bracketing it.
            self.publish_tool_started(NATIVE_WEB_SEARCH_TOOL_NAME, tool_call_id)
                .await;
            self.publish_tool_completed(NATIVE_WEB_SEARCH_TOOL_NAME, tool_call_id)
                .await;

            if let Some(query) = native_web_search_query(item) {
                // Kept out of the info log above — same split as `slack_auto_response_discard*`.
                self.logger.debug(
                    "llm_native_web_search_call_query",
                    log_fields([
                        ("ownerId", LogFieldValue::from(self.owner_id.clone())),
                        ("toolCallId", LogFieldValue::from(tool_call_id.to_string())),
                        ("query", LogFieldValue::from(query)),
                    ]),
                );
            }
        }
    }

    pub(crate) async fn handle_tool_call(
        &self,
        tool_name: &str,
        tool_call_id: Option<&str>,
        internal_call_id: &str,
    ) -> ToolCallAction {
        if self.is_cancelled() {
            return ToolCallAction::stop(TASK_CANCELLED_REASON);
        }
        let task_id = tool_call_id.unwrap_or(internal_call_id);
        self.log_tool_started(tool_name, task_id);
        self.publish_tool_started(tool_name, task_id).await;
        ToolCallAction::run()
    }

    pub(crate) async fn handle_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: Option<&str>,
        internal_call_id: &str,
        raw_result: &ToolResult,
    ) -> ToolResultAction {
        let task_id = tool_call_id.unwrap_or(internal_call_id);
        self.log_tool_completed(tool_name, task_id, raw_result);
        self.publish_tool_completed(tool_name, task_id).await;
        if self.is_cancelled() {
            return ToolResultAction::stop(TASK_CANCELLED_REASON);
        }
        ToolResultAction::keep()
    }

    async fn handle_observation(&self) -> ObservationAction {
        if self.is_cancelled() {
            return ObservationAction::stop(TASK_CANCELLED_REASON);
        }
        ObservationAction::continue_run()
    }
}

fn classify_tool_result(result: &ToolResult) -> &'static str {
    if result.is_error() || result.is_refused() {
        "error"
    } else {
        "success"
    }
}

/// Extracts a `web_search_call` output item's query text from `action.queries` — e.g.
/// `{"action": {"type": "search", "queries": ["rig framework"]}}` — joining multiple queries with
/// `; `. `None` when the item carries no queries (the shape varies by action type).
fn native_web_search_query(item: &serde_json::Value) -> Option<String> {
    let queries = item.get("action")?.get("queries")?.as_array()?;
    let joined = queries
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>()
        .join("; ");

    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

impl AgentHook for AgentExecutionHook {
    async fn on_completion_call(
        &self,
        _ctx: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        self.handle_completion_call().await
    }

    async fn on_completion_response(
        &self,
        _ctx: &HookContext,
        event: CompletionResponseEvent<'_>,
    ) -> ObservationAction {
        self.handle_completion_response(event.usage, event.raw)
            .await
    }

    async fn on_tool_call(&self, _ctx: &HookContext, event: ToolCall<'_>) -> ToolCallAction {
        self.handle_tool_call(event.tool_name, event.tool_call_id, event.internal_call_id)
            .await
    }

    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        self.handle_tool_result(
            event.tool_name,
            event.tool_call_id,
            event.internal_call_id,
            event.raw_result,
        )
        .await
    }

    async fn on_text_delta(&self, _ctx: &HookContext, _event: TextDelta<'_>) -> ObservationAction {
        self.handle_observation().await
    }

    async fn on_tool_call_delta(
        &self,
        _ctx: &HookContext,
        _event: ToolCallDelta<'_>,
    ) -> ObservationAction {
        self.handle_observation().await
    }

    async fn on_stream_response_finish(
        &self,
        _ctx: &HookContext,
        _event: StreamResponseFinish<'_>,
    ) -> ObservationAction {
        self.handle_observation().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::logger::{LogEntry, LogFieldValue, LogLevel, Logger, MockLogger};
    use reili_core::task::{
        MockTaskProgressEventPort, TaskCancellation, TaskProgressEvent, TaskProgressEventInput,
        TaskRuntime,
    };
    use rig::agent::{ObservationAction, ToolCallAction, ToolResultAction};
    use rig::tool::{ToolExecutionError, ToolOutput, ToolResult};

    use super::AgentExecutionHook;
    use crate::outbound::agents::runner::usage_collector::LlmUsageCollector;

    struct LoggerHarness {
        inner: MockLogger,
    }

    impl Logger for LoggerHarness {
        fn log(&self, entry: LogEntry) {
            self.inner.log(entry);
        }
    }

    fn sample_runtime() -> TaskRuntime {
        TaskRuntime {
            started_at_iso: "2026-03-28T00:00:00.000Z".to_string(),
            channel: "C123".to_string(),
            thread_ts: "1710000000.123456".to_string(),
            retry_count: 2,
        }
    }

    fn sample_cancellation() -> TaskCancellation {
        TaskCancellation::new()
    }

    fn logger_with_entries(entries: Arc<Mutex<Vec<LogEntry>>>, times: usize) -> Arc<dyn Logger> {
        let mut inner = MockLogger::new();
        inner.expect_log().times(times).returning(move |entry| {
            entries.lock().expect("lock entries").push(entry);
        });

        Arc::new(LoggerHarness { inner })
    }

    fn field_contains(
        fields: &std::collections::BTreeMap<String, LogFieldValue>,
        needle: &str,
    ) -> bool {
        fields.values().any(|value| match value {
            LogFieldValue::String(content) => content.contains(needle),
            _ => false,
        })
    }

    #[tokio::test]
    async fn publishes_tool_started_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::new(Mutex::new(Vec::new())), 0),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_started("search_datadog_logs", "task-1")
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[TaskProgressEventInput {
                owner_id: "datadog_agent".to_string(),
                event: TaskProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "search_datadog_logs".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn publishes_tool_completed_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::new(Mutex::new(Vec::new())), 0),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_completed("search_datadog_metrics", "task-2")
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[TaskProgressEventInput {
                owner_id: "datadog_agent".to_string(),
                event: TaskProgressEvent::ToolCallCompleted {
                    task_id: "task-2".to_string(),
                    title: "search_datadog_metrics".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn ignores_report_progress_tool_events_for_progress_updates() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port.expect_publish().times(0);
        let hook = AgentExecutionHook::new(
            "github_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(log_entries, 2),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let started_action = hook
            .handle_tool_call("report_progress", Some("task-3"), "internal-1")
            .await;
        let completed_action = hook
            .handle_tool_result(
                "report_progress",
                Some("task-3"),
                "internal-1",
                &ToolResult::success(ToolOutput::text("done")),
            )
            .await;

        assert_eq!(started_action, ToolCallAction::Run);
        assert_eq!(completed_action, ToolResultAction::Keep);
    }

    #[test]
    fn tracks_requests_and_usage() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port.expect_publish().times(0);
        let collector = LlmUsageCollector::new();
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(log_entries, 0),
            Arc::new(progress_event_port),
            collector.clone(),
        );

        hook.track_completion_call();
        hook.track_completion_response(rig::completion::Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        });

        assert_eq!(collector.snapshot().requests, 1);
        assert_eq!(collector.snapshot().total_tokens, 30);
    }

    #[tokio::test]
    async fn logs_and_publishes_progress_for_native_web_search_call() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let progress_calls = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&progress_calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(2)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 2),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.handle_native_web_search_calls(&serde_json::json!({
            "output": [
                {
                    "type": "web_search_call",
                    "id": "ws_001",
                    "status": "completed",
                    "action": { "type": "search", "queries": ["rig framework"] },
                },
            ],
        }))
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].event, "llm_native_web_search_call_observed");
        assert_eq!(entries[0].level, LogLevel::Info);
        assert_eq!(
            entries[0]
                .fields
                .get("toolCallId")
                .and_then(LogFieldValue::as_str),
            Some("ws_001")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("status")
                .and_then(LogFieldValue::as_str),
            Some("completed")
        );
        assert!(!entries[0].fields.contains_key("query"));

        assert_eq!(entries[1].event, "llm_native_web_search_call_query");
        assert_eq!(entries[1].level, LogLevel::Debug);
        assert_eq!(
            entries[1]
                .fields
                .get("toolCallId")
                .and_then(LogFieldValue::as_str),
            Some("ws_001")
        );
        assert_eq!(
            entries[1]
                .fields
                .get("query")
                .and_then(LogFieldValue::as_str),
            Some("rig framework")
        );

        assert_eq!(
            progress_calls.lock().expect("lock calls").as_slice(),
            &[
                TaskProgressEventInput {
                    owner_id: "datadog_agent".to_string(),
                    event: TaskProgressEvent::ToolCallStarted {
                        task_id: "ws_001".to_string(),
                        title: "web_search".to_string(),
                    },
                },
                TaskProgressEventInput {
                    owner_id: "datadog_agent".to_string(),
                    event: TaskProgressEvent::ToolCallCompleted {
                        task_id: "ws_001".to_string(),
                        title: "web_search".to_string(),
                    },
                },
            ]
        );
    }

    #[tokio::test]
    async fn omits_debug_query_log_when_action_carries_no_queries() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(2)
            .returning(|_| Ok(()));
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.handle_native_web_search_calls(&serde_json::json!({
            "output": [
                { "type": "web_search_call", "id": "ws_002", "status": "in_progress" },
            ],
        }))
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].event, "llm_native_web_search_call_observed");
    }

    #[tokio::test]
    async fn does_not_log_or_publish_when_raw_response_output_has_no_web_search_call() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port.expect_publish().times(0);
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 0),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.handle_native_web_search_calls(&serde_json::json!({
            "output": [
                { "type": "message", "id": "msg_1" },
            ],
        }))
        .await;

        assert!(log_entries.lock().expect("lock entries").is_empty());
    }

    #[tokio::test]
    async fn logs_tool_started_without_args() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let action = hook
            .handle_tool_call("search_datadog_logs", Some("task-1"), "internal-1")
            .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(action, ToolCallAction::Run);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, LogLevel::Info);
        assert_eq!(entries[0].event, "llm_tool_execution_started");
        assert_eq!(
            entries[0]
                .fields
                .get("ownerId")
                .and_then(LogFieldValue::as_str),
            Some("datadog_agent")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("toolName")
                .and_then(LogFieldValue::as_str),
            Some("search_datadog_logs")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("toolCallId")
                .and_then(LogFieldValue::as_str),
            Some("task-1")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("channel")
                .and_then(LogFieldValue::as_str),
            Some("C123")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("threadTs")
                .and_then(LogFieldValue::as_str),
            Some("1710000000.123456")
        );
        assert_eq!(
            entries[0]
                .fields
                .get("retryCount")
                .and_then(LogFieldValue::as_u64),
            Some(2)
        );
        assert!(!entries[0].fields.contains_key("args"));
        assert!(!field_contains(
            &entries[0].fields,
            "{\"query\":\"service:payments @message:error\"}",
        ));
    }

    #[tokio::test]
    async fn logs_tool_completed_success_without_result_body() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let action = hook
            .handle_tool_result(
                "search_datadog_logs",
                Some("task-1"),
                "internal-1",
                &ToolResult::success(ToolOutput::text("sensitive output body")),
            )
            .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(action, ToolResultAction::Keep);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, LogLevel::Info);
        assert_eq!(entries[0].event, "llm_tool_execution_completed");
        assert_eq!(
            entries[0]
                .fields
                .get("result")
                .and_then(LogFieldValue::as_str),
            Some("success")
        );
        assert!(!entries[0].fields.contains_key("args"));
        assert!(!entries[0].fields.contains_key("resultBody"));
        assert!(!field_contains(
            &entries[0].fields,
            "\"sensitive output body\""
        ));
    }

    #[tokio::test]
    async fn logs_tool_completed_error_result_as_error() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.handle_tool_result(
            "search_datadog_logs",
            Some("task-1"),
            "internal-1",
            &ToolResult::failed(ToolExecutionError::permission_denied("permission denied")),
        )
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(
            entries[0]
                .fields
                .get("result")
                .and_then(LogFieldValue::as_str),
            Some("error")
        );
    }

    #[tokio::test]
    async fn logs_tool_completed_refusal_as_error() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.handle_tool_result(
            "search_datadog_logs",
            Some("task-1"),
            "internal-1",
            &ToolResult::failed(ToolExecutionError::refused("refused")),
        )
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(
            entries[0]
                .fields
                .get("result")
                .and_then(LogFieldValue::as_str),
            Some("error")
        );
    }

    #[tokio::test]
    async fn observation_hooks_continue_when_not_cancelled() {
        let hook = AgentExecutionHook::new(
            "datadog_agent".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::new(Mutex::new(Vec::new())), 0),
            Arc::new(MockTaskProgressEventPort::new()),
            LlmUsageCollector::new(),
        );

        assert_eq!(hook.handle_observation().await, ObservationAction::Continue);
    }
}
