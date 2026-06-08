use std::sync::Arc;

use chrono::Utc;
use reili_core::error::AgentRunFailedError;
use reili_core::logger::Logger;
use reili_core::task::{
    LlmExecutionMetadata, LlmUsageSnapshot, RunTaskInput, TASK_RUNNER_PROGRESS_OWNER_ID,
    TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort, TaskRunOutcome,
    TaskRunReport,
};
use rig::completion::{Prompt, PromptError};
use rig::prelude::CompletionClient;

use super::execution_hook::AgentExecutionHook;
use super::provider_settings::LlmProviderSettings;
use super::usage_collector::LlmUsageCollector;
use crate::outbound::agents::connector::{
    ConnectorPrepareError, ConnectorPromptFact, ConnectorSet,
};
use crate::outbound::agents::task_agent::{
    AgentInstructionsConfig, BuildTaskAgentInput, BuildTaskPromptInput,
    CreateTaskAgentFactoryInput, TaskAgentConfig, TaskAgentExecutionContext, TaskAgentFactory,
    TaskAgentRunContext, build_task_prompt,
};

pub struct RunLlmTaskRunnerInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
    pub run: RunTaskInput,
}

pub async fn run_task<C>(
    input: RunLlmTaskRunnerInput<C>,
) -> Result<TaskRunOutcome, AgentRunFailedError>
where
    C: CompletionClient + Clone + Send + Sync + 'static,
    C::CompletionModel: 'static,
{
    if input.run.context.cancellation.is_cancelled() {
        return Ok(TaskRunOutcome::Cancelled);
    }

    let usage_collector = LlmUsageCollector::new();
    let runtime = input.run.context.runtime.clone();
    let task_runner_prompt_hook = create_task_runner_prompt_hook(CreateTaskRunnerPromptHookInput {
        cancellation: input.run.context.cancellation.clone(),
        logger: Arc::clone(&input.run.logger),
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        runtime: runtime.clone(),
        usage_collector: usage_collector.clone(),
    });
    let prepared_connectors = input.connectors.prepare_all().await.map_err(|error| {
        let usage = usage_collector.snapshot();
        match error {
            ConnectorPrepareError::ConnectionFailed { message } => {
                AgentRunFailedError::new_permanent(usage, message)
            }
            ConnectorPrepareError::Other(port_error) => {
                create_failed_error(CreateTaskRunnerFailedErrorInput {
                    usage,
                    cause_message: port_error.message,
                })
            }
        }
    })?;
    let prompt_facts: Vec<ConnectorPromptFact> = prepared_connectors
        .iter()
        .flat_map(|connector| connector.prompt_facts())
        .collect();
    let memory_items = input.run.request.memory_items.clone();
    let slack_action_token = input.run.request.trigger_message.action_token.clone();
    let task_prompt = build_task_prompt(BuildTaskPromptInput {
        request: input.run.request,
        now: Utc::now(),
        runtime: runtime.clone(),
        language: input.language.clone(),
        prompt_facts,
    });
    let task_agent_factory = TaskAgentFactory::new(CreateTaskAgentFactoryInput {
        client: input.client,
        config: TaskAgentConfig {
            settings: input.settings.clone(),
            instructions: AgentInstructionsConfig {
                language: input.language,
                additional_system_prompt: input.additional_system_prompt,
            },
        },
    });
    let task_agent = task_agent_factory.build(BuildTaskAgentInput {
        run_context: TaskAgentRunContext {
            resources: Arc::new(input.run.context.resources),
            execution: TaskAgentExecutionContext {
                logger: Arc::clone(&input.run.logger),
                runtime,
                cancellation: input.run.context.cancellation.clone(),
                on_progress_event: Arc::clone(&input.run.on_progress_event),
                usage_collector: usage_collector.clone(),
            },
            slack_action_token,
            memory_items,
        },
        prepared_connectors,
    });

    let prompt_response_result = task_agent
        .prompt(task_prompt)
        .max_turns(input.settings.task_runner_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(task_runner_prompt_hook)
        .extended_details()
        .await;

    let prompt_response = match prompt_response_result {
        Ok(response) => response,
        Err(PromptError::PromptCancelled { .. }) => return Ok(TaskRunOutcome::Cancelled),
        Err(error) => {
            return Err(create_failed_error(CreateTaskRunnerFailedErrorInput {
                usage: usage_collector.snapshot(),
                cause_message: error.to_string(),
            }));
        }
    };

    if input.run.context.cancellation.is_cancelled() {
        return Ok(TaskRunOutcome::Cancelled);
    }

    publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        usage: usage_collector.snapshot(),
    })
    .await?;

    Ok(TaskRunOutcome::Succeeded(TaskRunReport {
        result_text: prompt_response.output,
        usage: usage_collector.snapshot(),
        execution: LlmExecutionMetadata {
            provider: input.settings.provider,
            model: input.settings.task_runner_model,
        },
    }))
}

struct PublishMessageOutputCreatedEventInput {
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    usage: LlmUsageSnapshot,
}

struct CreateTaskRunnerPromptHookInput {
    cancellation: reili_core::task::TaskCancellation,
    logger: Arc<dyn Logger>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    runtime: reili_core::task::TaskRuntime,
    usage_collector: LlmUsageCollector,
}

fn create_task_runner_prompt_hook(input: CreateTaskRunnerPromptHookInput) -> AgentExecutionHook {
    AgentExecutionHook::new(
        TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
        input.runtime,
        input.cancellation,
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
        MockTaskProgressEventPort, TASK_RUNNER_PROGRESS_OWNER_ID, TaskCancellation,
        TaskProgressEvent, TaskProgressEventInput, TaskRuntime,
    };
    use rig::agent::{PromptHook, ToolCallHookAction};
    use rig::providers::openai;

    use super::{CreateTaskRunnerPromptHookInput, create_task_runner_prompt_hook};
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
            cancellation: TaskCancellation::new(),
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
            cancellation: TaskCancellation::new(),
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
