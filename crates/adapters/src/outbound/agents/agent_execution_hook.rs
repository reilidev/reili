use std::future::Future;
use std::sync::Arc;

use reili_core::logger::{LogFieldValue, Logger, log_fields};
use reili_core::task::{
    TaskCancellation, TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort, TaskRuntime,
};
use rig::agent::{HookAction, PromptHook, ToolCallHookAction};
use rig::completion::CompletionModel;
use rig::message::Message;

use super::llm_usage_collector::LlmUsageCollector;

const REPORT_PROGRESS_TOOL_NAME: &str = "report_progress";
const TOOL_RESULT_ERROR_PREFIXES: [&str; 2] = ["ToolCallError:", "JsonError:"];
const TASK_CANCELLED_REASON: &str = "task_cancelled";

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

    fn log_tool_completed(&self, tool_name: &str, task_id: &str, raw_result: &str) {
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
}

fn classify_tool_result(raw_result: &str) -> &'static str {
    if TOOL_RESULT_ERROR_PREFIXES
        .iter()
        .any(|prefix| raw_result.starts_with(prefix))
    {
        "error"
    } else {
        "success"
    }
}

impl<M> PromptHook<M> for AgentExecutionHook
where
    M: CompletionModel,
{
    fn on_completion_call(
        &self,
        _prompt: &Message,
        _history: &[Message],
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            hook.track_completion_call();
            HookAction::cont()
        }
    }

    fn on_completion_response(
        &self,
        _prompt: &Message,
        response: &rig::completion::CompletionResponse<M::Response>,
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();
        let usage = response.usage;

        async move {
            hook.track_completion_response(usage);
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            HookAction::cont()
        }
    }

    fn on_tool_call(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
    ) -> impl Future<Output = ToolCallHookAction> + Send {
        let hook = self.clone();
        let task_id = tool_call_id.unwrap_or_else(|| internal_call_id.to_string());
        let tool_name = tool_name.to_string();

        async move {
            if hook.is_cancelled() {
                return ToolCallHookAction::terminate(TASK_CANCELLED_REASON);
            }
            hook.log_tool_started(&tool_name, &task_id);
            hook.publish_tool_started(&tool_name, &task_id).await;
            ToolCallHookAction::cont()
        }
    }

    fn on_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
        result: &str,
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();
        let task_id = tool_call_id.unwrap_or_else(|| internal_call_id.to_string());
        let tool_name = tool_name.to_string();
        let result = result.to_string();

        async move {
            hook.log_tool_completed(&tool_name, &task_id, &result);
            hook.publish_tool_completed(&tool_name, &task_id).await;
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            HookAction::cont()
        }
    }

    fn on_text_delta(
        &self,
        _text_delta: &str,
        _aggregated_text: &str,
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            HookAction::cont()
        }
    }

    fn on_tool_call_delta(
        &self,
        _tool_call_id: &str,
        _internal_call_id: &str,
        _tool_name: Option<&str>,
        _tool_call_delta: &str,
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            HookAction::cont()
        }
    }

    fn on_stream_completion_response_finish(
        &self,
        _prompt: &Message,
        _response: &<M as CompletionModel>::StreamingResponse,
    ) -> impl Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            if hook.is_cancelled() {
                return HookAction::terminate(TASK_CANCELLED_REASON);
            }
            HookAction::cont()
        }
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
    use rig::agent::{PromptHook, ToolCallHookAction};
    use rig::providers::openai;

    use super::AgentExecutionHook;
    use crate::outbound::agents::llm_usage_collector::LlmUsageCollector;

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
            "investigate_datadog".to_string(),
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
                owner_id: "investigate_datadog".to_string(),
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
            "investigate_datadog".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::new(Mutex::new(Vec::new())), 0),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_completed("query_datadog_metrics", "task-2")
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[TaskProgressEventInput {
                owner_id: "investigate_datadog".to_string(),
                event: TaskProgressEvent::ToolCallCompleted {
                    task_id: "task-2".to_string(),
                    title: "query_datadog_metrics".to_string(),
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
            "investigate_github".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(log_entries, 2),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let started_action = <_ as PromptHook<openai::CompletionModel>>::on_tool_call(
            &hook,
            "report_progress",
            Some("task-3".to_string()),
            "internal-1",
            "{\"title\":\"Inspect logs\"}",
        )
        .await;
        let completed_action = <_ as PromptHook<openai::CompletionModel>>::on_tool_result(
            &hook,
            "report_progress",
            Some("task-3".to_string()),
            "internal-1",
            "{\"title\":\"Inspect logs\"}",
            "\"done\"",
        )
        .await;

        assert_eq!(started_action, ToolCallHookAction::Continue);
        assert_eq!(completed_action, rig::agent::HookAction::Continue);
    }

    #[test]
    fn tracks_requests_and_usage() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port.expect_publish().times(0);
        let collector = LlmUsageCollector::new();
        let hook = AgentExecutionHook::new(
            "investigate_datadog".to_string(),
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
        });

        assert_eq!(collector.snapshot().requests, 1);
        assert_eq!(collector.snapshot().total_tokens, 30);
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
            "investigate_datadog".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let action = <_ as PromptHook<openai::CompletionModel>>::on_tool_call(
            &hook,
            "search_datadog_logs",
            Some("task-1".to_string()),
            "internal-1",
            "{\"query\":\"service:payments @message:error\"}",
        )
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(action, ToolCallHookAction::Continue);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, LogLevel::Info);
        assert_eq!(entries[0].event, "llm_tool_execution_started");
        assert_eq!(
            entries[0]
                .fields
                .get("ownerId")
                .and_then(LogFieldValue::as_str),
            Some("investigate_datadog")
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
            "investigate_datadog".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        let action = <_ as PromptHook<openai::CompletionModel>>::on_tool_result(
            &hook,
            "search_datadog_logs",
            Some("task-1".to_string()),
            "internal-1",
            "{\"query\":\"service:payments @message:error\"}",
            "\"sensitive output body\"",
        )
        .await;

        let entries = log_entries.lock().expect("lock entries");
        assert_eq!(action, rig::agent::HookAction::Continue);
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
    async fn logs_tool_completed_tool_call_error_as_error() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let hook = AgentExecutionHook::new(
            "investigate_datadog".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        <_ as PromptHook<openai::CompletionModel>>::on_tool_result(
            &hook,
            "search_datadog_logs",
            Some("task-1".to_string()),
            "internal-1",
            "{}",
            "ToolCallError: permission denied",
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
    async fn logs_tool_completed_json_error_as_error() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let hook = AgentExecutionHook::new(
            "investigate_datadog".to_string(),
            sample_runtime(),
            sample_cancellation(),
            logger_with_entries(Arc::clone(&log_entries), 1),
            Arc::new(progress_event_port),
            LlmUsageCollector::new(),
        );

        <_ as PromptHook<openai::CompletionModel>>::on_tool_result(
            &hook,
            "search_datadog_logs",
            Some("task-1".to_string()),
            "internal-1",
            "{}",
            "JsonError: missing field",
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
}
