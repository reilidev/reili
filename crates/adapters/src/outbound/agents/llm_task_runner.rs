use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::task::{
    LlmExecutionMetadata, LlmUsageSnapshot, RunTaskInput, TASK_RUNNER_PROGRESS_OWNER_ID,
    TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort, TaskRunReport,
};
use rig::completion::Prompt;
use rig::prelude::CompletionClient;

use super::datadog_mcp_tools::{DatadogMcpToolConfig, connect_datadog_mcp_toolset};
use super::llm_provider_settings::LlmProviderSettings;
use super::llm_usage_collector::LlmUsageCollector;
use super::llm_usage_tracking_hook::LlmUsageTrackingHook;
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
    let usage_tracking_hook = LlmUsageTrackingHook::new(usage_collector.clone());
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
        runtime: input.run.context.runtime,
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        language: input.language,
        usage_collector: usage_collector.clone(),
    });

    let prompt_response = task_agent
        .prompt(task_prompt)
        .max_turns(input.settings.task_runner_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(usage_tracking_hook)
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
