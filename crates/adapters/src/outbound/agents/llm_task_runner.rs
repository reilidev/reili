use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::logger::Logger;
use reili_core::task::{
    LlmExecutionMetadata, LlmUsageSnapshot, RunTaskInput, TASK_RUNNER_PROGRESS_OWNER_ID,
    TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort, TaskRunReport,
};
use rig::completion::Prompt;
use rig::prelude::CompletionClient;

use super::agent_execution_hook::AgentExecutionHook;
use super::datadog_mcp_tools::{DatadogMcpToolConfig, connect_datadog_mcp_toolset};
use super::llm_provider_settings::LlmProviderSettings;
use super::llm_usage_collector::LlmUsageCollector;
use super::task_agents::{BuildTaskAgentInput, build_task_agent, build_task_prompt};

pub struct RunLlmTaskInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_scope_org: String,
    pub language: String,
    pub run: RunTaskInput,
}

pub async fn run_llm_task<C>(
    input: RunLlmTaskInput<C>,
) -> Result<TaskRunReport, AgentRunFailedError>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    let usage_collector = LlmUsageCollector::new();
    let runtime = input.run.context.runtime.clone();
    let task_runner_prompt_hook = create_task_runner_prompt_hook(CreateTaskRunnerPromptHookInput {
        logger: Arc::clone(&input.run.logger),
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        runtime: runtime.clone(),
        usage_collector: usage_collector.clone(),
    });
    let datadog_mcp_toolset = connect_datadog_mcp_toolset(&input.datadog_mcp)
        .await
        .map_err(|error| {
            let usage = usage_collector.snapshot();
            if error.is_connection_failed() {
                return AgentRunFailedError::new_permanent(usage, error.message);
            }

            create_failed_error(CreateTaskRunnerFailedErrorInput {
                usage,
                cause_message: error.message,
            })
        })?;
    let task_prompt = build_task_prompt(&input.run.request);
    let task_agent = build_task_agent(BuildTaskAgentInput {
        client: input.client,
        settings: input.settings.clone(),
        resources: Arc::new(input.run.context.resources),
        datadog_site: input.datadog_mcp.site.clone(),
        datadog_mcp_toolset,
        github_scope_org: input.github_scope_org,
        logger: Arc::clone(&input.run.logger),
        runtime,
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        language: input.language,
        usage_collector: usage_collector.clone(),
    });

    let prompt_response = task_agent
        .prompt(task_prompt)
        .max_turns(input.settings.task_runner_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(task_runner_prompt_hook)
        .extended_details()
        .await
        .map_err(|error| {
            create_failed_error(CreateTaskRunnerFailedErrorInput {
                usage: usage_collector.snapshot(),
                cause_message: error.to_string(),
            })
        })?;

    publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        usage: usage_collector.snapshot(),
    })
    .await?;

    Ok(TaskRunReport {
        result_text: prompt_response.output,
        usage: usage_collector.snapshot(),
        execution: LlmExecutionMetadata {
            provider: input.settings.provider,
            model: input.settings.task_runner_model,
        },
    })
}

struct PublishMessageOutputCreatedEventInput {
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    usage: LlmUsageSnapshot,
}

struct CreateTaskRunnerPromptHookInput {
    logger: Arc<dyn Logger>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    runtime: reili_core::task::TaskRuntime,
    usage_collector: LlmUsageCollector,
}

fn create_task_runner_prompt_hook(input: CreateTaskRunnerPromptHookInput) -> AgentExecutionHook {
    AgentExecutionHook::new(
        TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
        input.runtime,
        input.logger,
        input.on_progress_event,
        input.usage_collector,
    )
}

async fn publish_message_output_created_event(
    input: PublishMessageOutputCreatedEventInput,
) -> Result<(), AgentRunFailedError> {
    input
        .on_progress_event
        .publish(TaskProgressEventInput {
            owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
            event: TaskProgressEvent::MessageOutputCreated,
        })
        .await
        .map_err(|error| {
            create_failed_error(CreateTaskRunnerFailedErrorInput {
                usage: input.usage,
                cause_message: format!("Failed to publish progress event: {error}"),
            })
        })?;

    Ok(())
}

struct CreateTaskRunnerFailedErrorInput {
    usage: LlmUsageSnapshot,
    cause_message: String,
}

fn create_failed_error(input: CreateTaskRunnerFailedErrorInput) -> AgentRunFailedError {
    AgentRunFailedError::new(input.usage, input.cause_message)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::logger::{LogEntry, Logger, MockLogger};
    use reili_core::task::{
        MockTaskProgressEventPort, TASK_RUNNER_PROGRESS_OWNER_ID, TaskProgressEvent,
        TaskProgressEventInput, TaskRuntime,
    };
    use rig::agent::{PromptHook, ToolCallHookAction};
    use rig::providers::openai;

    use super::{CreateTaskRunnerPromptHookInput, create_task_runner_prompt_hook};
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
            retry_count: 0,
        }
    }

    fn logger_with_log_count(times: usize) -> Arc<dyn Logger> {
        let mut inner = MockLogger::new();
        inner.expect_log().times(times).returning(|_| ());
        Arc::new(LoggerHarness { inner })
    }

    #[tokio::test]
    async fn task_runner_prompt_hook_publishes_direct_datadog_tool_calls() {
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
        let hook = create_task_runner_prompt_hook(CreateTaskRunnerPromptHookInput {
            logger: logger_with_log_count(1),
            on_progress_event: Arc::new(progress_event_port),
            runtime: sample_runtime(),
            usage_collector: LlmUsageCollector::new(),
        });

        let action = <_ as PromptHook<openai::CompletionModel>>::on_tool_call(
            &hook,
            "search_datadog_services",
            Some("task-1".to_string()),
            "internal-1",
            "{}",
        )
        .await;

        assert_eq!(action, ToolCallHookAction::Continue);
        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[TaskProgressEventInput {
                owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
                event: TaskProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "search_datadog_services".to_string(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn task_runner_prompt_hook_ignores_report_progress_tool_calls() {
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port.expect_publish().times(0);
        let hook = create_task_runner_prompt_hook(CreateTaskRunnerPromptHookInput {
            logger: logger_with_log_count(1),
            on_progress_event: Arc::new(progress_event_port),
            runtime: sample_runtime(),
            usage_collector: LlmUsageCollector::new(),
        });

        let action = <_ as PromptHook<openai::CompletionModel>>::on_tool_call(
            &hook,
            "report_progress",
            Some("task-1".to_string()),
            "internal-1",
            "{}",
        )
        .await;

        assert_eq!(action, ToolCallHookAction::Continue);
    }
}
