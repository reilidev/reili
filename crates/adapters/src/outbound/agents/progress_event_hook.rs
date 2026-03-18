use std::sync::Arc;

use reili_core::investigation::{
    InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
};
use rig::agent::{HookAction, PromptHook};
use rig::completion::CompletionModel;
use rig::message::Message;

use super::llm_usage_collector::LlmUsageCollector;

const REPORT_PROGRESS_TOOL_NAME: &str = "report_progress";

#[derive(Clone)]
pub struct ProgressEventHook {
    owner_id: String,
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    usage_collector: LlmUsageCollector,
}

impl ProgressEventHook {
    pub fn new(
        owner_id: String,
        on_progress_event: Arc<dyn InvestigationProgressEventPort>,
        usage_collector: LlmUsageCollector,
    ) -> Self {
        Self {
            owner_id,
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
            .publish(InvestigationProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: InvestigationProgressEvent::ToolCallStarted {
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
            .publish(InvestigationProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: InvestigationProgressEvent::ToolCallCompleted {
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

    fn track_completion_call(&self) {
        self.usage_collector.record_request();
    }

    fn track_completion_response(&self, usage: rig::completion::Usage) {
        self.usage_collector.record_usage(&usage);
    }
}

impl<M> PromptHook<M> for ProgressEventHook
where
    M: CompletionModel,
{
    fn on_completion_call(
        &self,
        _prompt: &Message,
        _history: &[Message],
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let hook = self.clone();

        async move {
            hook.track_completion_call();
            HookAction::cont()
        }
    }

    fn on_completion_response(
        &self,
        _prompt: &Message,
        response: &rig::completion::CompletionResponse<M::Response>,
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let hook = self.clone();
        let usage = response.usage;

        async move {
            hook.track_completion_response(usage);
            HookAction::cont()
        }
    }

    fn on_tool_call(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
    ) -> impl std::future::Future<Output = rig::agent::ToolCallHookAction> + Send {
        let hook = self.clone();
        let task_id = tool_call_id.unwrap_or_else(|| internal_call_id.to_string());
        let tool_name = tool_name.to_string();

        async move {
            hook.publish_tool_started(&tool_name, &task_id).await;
            rig::agent::ToolCallHookAction::cont()
        }
    }

    fn on_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        _args: &str,
        _result: &str,
    ) -> impl std::future::Future<Output = HookAction> + Send {
        let hook = self.clone();
        let task_id = tool_call_id.unwrap_or_else(|| internal_call_id.to_string());
        let tool_name = tool_name.to_string();

        async move {
            hook.publish_tool_completed(&tool_name, &task_id).await;
            HookAction::cont()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::investigation::{
        InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
    };

    use super::ProgressEventHook;
    use crate::outbound::agents::llm_usage_collector::LlmUsageCollector;

    struct MockProgressEventPort {
        calls: Arc<Mutex<Vec<InvestigationProgressEventInput>>>,
    }

    #[async_trait]
    impl InvestigationProgressEventPort for MockProgressEventPort {
        async fn publish(&self, input: InvestigationProgressEventInput) -> Result<(), PortError> {
            self.calls.lock().expect("lock calls").push(input);
            Ok(())
        }
    }

    #[tokio::test]
    async fn publishes_tool_started_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let hook = ProgressEventHook::new(
            "investigate_datadog".to_string(),
            Arc::new(MockProgressEventPort {
                calls: Arc::clone(&calls),
            }),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_started("search_datadog_logs", "task-1")
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[InvestigationProgressEventInput {
                owner_id: "investigate_datadog".to_string(),
                event: InvestigationProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "search_datadog_logs".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn publishes_tool_completed_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let hook = ProgressEventHook::new(
            "investigate_datadog".to_string(),
            Arc::new(MockProgressEventPort {
                calls: Arc::clone(&calls),
            }),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_completed("query_datadog_metrics", "task-2")
            .await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[InvestigationProgressEventInput {
                owner_id: "investigate_datadog".to_string(),
                event: InvestigationProgressEvent::ToolCallCompleted {
                    task_id: "task-2".to_string(),
                    title: "query_datadog_metrics".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn ignores_report_progress_tool_events() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let hook = ProgressEventHook::new(
            "investigate_github".to_string(),
            Arc::new(MockProgressEventPort {
                calls: Arc::clone(&calls),
            }),
            LlmUsageCollector::new(),
        );

        hook.publish_tool_started("report_progress", "task-3").await;
        hook.publish_tool_completed("report_progress", "task-3")
            .await;

        assert!(calls.lock().expect("lock calls").is_empty());
    }

    #[test]
    fn tracks_requests_and_usage() {
        let collector = LlmUsageCollector::new();
        let hook = ProgressEventHook::new(
            "investigate_datadog".to_string(),
            Arc::new(MockProgressEventPort {
                calls: Arc::new(Mutex::new(Vec::new())),
            }),
            collector.clone(),
        );

        hook.track_completion_call();
        hook.track_completion_response(rig::completion::Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cached_input_tokens: 0,
        });

        assert_eq!(collector.snapshot().requests, 1);
        assert_eq!(collector.snapshot().total_tokens, 30);
    }
}
