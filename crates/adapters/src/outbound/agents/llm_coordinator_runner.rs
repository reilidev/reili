use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    COORDINATOR_PROGRESS_OWNER_ID, CoordinatorRunReport, InvestigationProgressEvent,
    InvestigationProgressEventInput, InvestigationProgressEventPort, LlmExecutionMetadata,
    RunCoordinatorInput,
};
use rig::completion::{Prompt, Usage};
use rig::prelude::CompletionClient;

use super::investigation_agents::{
    BuildCoordinatorAgentInput, build_coordinator_agent, build_coordinator_prompt,
};
use super::llm_provider_settings::LlmProviderSettings;
use super::llm_usage_mapper::{MapRigUsageToSnapshotInput, map_rig_usage_to_llm_usage_snapshot};
use super::request_count_hook::RequestCountHook;

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
    let request_count_hook = RequestCountHook::new();
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
    });

    let prompt_response = coordinator_agent
        .prompt(coordinator_prompt)
        .max_turns(input.settings.coordinator_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(request_count_hook.clone())
        .extended_details()
        .await
        .map_err(|error| {
            create_failed_error(CreateCoordinatorRunnerFailedErrorInput {
                usage: None,
                requests: request_count_hook.request_count(),
                cause_message: error.to_string(),
            })
        })?;
    let usage = Some(prompt_response.usage);

    publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        usage,
        requests: request_count_hook.request_count(),
    })
    .await?;

    Ok(CoordinatorRunReport {
        result_text: prompt_response.output,
        usage: map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
            usage,
            requests: request_count_hook.request_count(),
        }),
        execution: LlmExecutionMetadata {
            provider: input.settings.provider,
            model: input.settings.coordinator_model,
        },
    })
}

struct PublishMessageOutputCreatedEventInput {
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    usage: Option<Usage>,
    requests: u32,
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
                requests: input.requests,
                cause_message: format!("Failed to publish progress event: {error}"),
            })
        })?;

    Ok(())
}

struct CreateCoordinatorRunnerFailedErrorInput {
    usage: Option<Usage>,
    requests: u32,
    cause_message: String,
}

fn create_failed_error(input: CreateCoordinatorRunnerFailedErrorInput) -> AgentRunFailedError {
    let usage = map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
        usage: input.usage,
        requests: input.requests,
    });

    AgentRunFailedError::new(usage, input.cause_message)
}
