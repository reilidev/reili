use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use reili_core::investigation::{
    INVESTIGATION_LEAD_PROGRESS_OWNER_ID, InvestigationLeadRunReport, InvestigationProgressEvent,
    InvestigationProgressEventInput, InvestigationProgressEventPort, LlmExecutionMetadata,
    LlmUsageSnapshot, RunInvestigationLeadInput,
};
use rig::completion::Prompt;
use rig::prelude::CompletionClient;

use super::datadog_mcp_tools::{DatadogMcpToolConfig, connect_datadog_mcp_toolset};
use super::investigation_agents::{
    BuildInvestigationLeadAgentInput, build_investigation_lead_agent,
    build_investigation_lead_prompt,
};
use super::llm_provider_settings::LlmProviderSettings;
use super::llm_usage_collector::LlmUsageCollector;
use super::llm_usage_tracking_hook::LlmUsageTrackingHook;

pub struct RunLlmInvestigationLeadInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub datadog_mcp: DatadogMcpToolConfig,
    pub github_scope_org: String,
    pub language: String,
    pub run: RunInvestigationLeadInput,
}

pub async fn run_llm_investigation_lead<C>(
    input: RunLlmInvestigationLeadInput<C>,
) -> Result<InvestigationLeadRunReport, AgentRunFailedError>
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

            create_failed_error(CreateInvestigationLeadRunnerFailedErrorInput {
                usage,
                cause_message: error.message,
            })
        })?;
    let investigation_lead_prompt = build_investigation_lead_prompt(&input.run.request);
    let investigation_lead_agent =
        build_investigation_lead_agent(BuildInvestigationLeadAgentInput {
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

    let prompt_response = investigation_lead_agent
        .prompt(investigation_lead_prompt)
        .max_turns(input.settings.investigation_lead_max_turns)
        .with_tool_concurrency(input.settings.tool_concurrency)
        .with_hook(usage_tracking_hook)
        .extended_details()
        .await
        .map_err(|error| {
            create_failed_error(CreateInvestigationLeadRunnerFailedErrorInput {
                usage: usage_collector.snapshot(),
                cause_message: error.to_string(),
            })
        })?;

    publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
        on_progress_event: Arc::clone(&input.run.on_progress_event),
        usage: usage_collector.snapshot(),
    })
    .await?;

    Ok(InvestigationLeadRunReport {
        result_text: prompt_response.output,
        usage: usage_collector.snapshot(),
        execution: LlmExecutionMetadata {
            provider: input.settings.provider,
            model: input.settings.investigation_lead_model,
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
            owner_id: INVESTIGATION_LEAD_PROGRESS_OWNER_ID.to_string(),
            event: InvestigationProgressEvent::MessageOutputCreated,
        })
        .await
        .map_err(|error| {
            create_failed_error(CreateInvestigationLeadRunnerFailedErrorInput {
                usage: input.usage,
                cause_message: format!("Failed to publish progress event: {error}"),
            })
        })?;

    Ok(())
}

struct CreateInvestigationLeadRunnerFailedErrorInput {
    usage: LlmUsageSnapshot,
    cause_message: String,
}

fn create_failed_error(
    input: CreateInvestigationLeadRunnerFailedErrorInput,
) -> AgentRunFailedError {
    AgentRunFailedError::new(input.usage, input.cause_message)
}
