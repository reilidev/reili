use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    COORDINATOR_PROGRESS_OWNER_ID, CoordinatorRunReport, InvestigationProgressEvent,
    InvestigationProgressEventInput, InvestigationProgressEventPort, LlmExecutionMetadata,
    LlmUsageSnapshot, RunCoordinatorInput,
};
use rig::completion::Prompt;
use rig::prelude::CompletionClient;

use super::investigation_agents::{
    BuildCoordinatorAgentInput, build_coordinator_agent, build_coordinator_prompt,
};
use super::llm_provider_settings::LlmProviderSettings;
use super::llm_usage_collector::LlmUsageCollector;
use super::llm_usage_tracking_hook::LlmUsageTrackingHook;

pub struct RunLlmCoordinatorInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub language: String,
    pub run: RunCoordinatorInput,
}

pub async fn run_llm_coordinator<C>(
    input: RunLlmCoordinatorInput<C>,
) -> Result<CoordinatorRunReport, AgentRunFailedError>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    let usage_collector = LlmUsageCollector::new();
    let usage_tracking_hook = LlmUsageTrackingHook::new(usage_collector.clone());
    let coordinator_prompt = build_coordinator_prompt(&input.run.alert_context);
    let coordinator_agent = build_coordinator_agent(BuildCoordinatorAgentInput {
        client: input.client,
        settings: input.settings.clone(),
        resources: Arc::new(input.run.context.resources),
        datadog_site: input.datadog_site,
        github_scope_org: input.github_scope_org,
        runtime: input.run.context.runtime,
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        language: input.language,
        usage_collector: usage_collector.clone(),
    });

    let prompt_response = coordinator_agent
        .prompt(coordinator_prompt)
        .max_turns(input.settings.coordinator_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(usage_tracking_hook)
        .extended_details()
        .await
        .map_err(|error| {
            create_failed_error(CreateCoordinatorRunnerFailedErrorInput {
                usage: usage_collector.snapshot(),
                cause_message: error.to_string(),
            })
        })?;

    publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        usage: usage_collector.snapshot(),
    })
    .await?;

    Ok(CoordinatorRunReport {
        result_text: prompt_response.output,
        usage: usage_collector.snapshot(),
        execution: LlmExecutionMetadata {
            provider: input.settings.provider,
            model: input.settings.coordinator_model,
        },
    })
}

struct PublishMessageOutputCreatedEventInput {
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    usage: LlmUsageSnapshot,
}

async fn publish_message_output_created_event(
    input: PublishMessageOutputCreatedEventInput,
) -> Result<(), AgentRunFailedError> {
    input
        .on_progress_event
        .publish(InvestigationProgressEventInput {
            owner_id: COORDINATOR_PROGRESS_OWNER_ID.to_string(),
            event: InvestigationProgressEvent::MessageOutputCreated,
        })
        .await
        .map_err(|error| {
            create_failed_error(CreateCoordinatorRunnerFailedErrorInput {
                usage: input.usage,
                cause_message: format!("Failed to publish progress event: {error}"),
            })
        })?;

    Ok(())
}

struct CreateCoordinatorRunnerFailedErrorInput {
    usage: LlmUsageSnapshot,
    cause_message: String,
}

fn create_failed_error(input: CreateCoordinatorRunnerFailedErrorInput) -> AgentRunFailedError {
    AgentRunFailedError::new(input.usage, input.cause_message)
}
