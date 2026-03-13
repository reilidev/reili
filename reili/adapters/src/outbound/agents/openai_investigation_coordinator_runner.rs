use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::errors::AgentRunFailedError;
use reili_shared::ports::outbound::{
    COORDINATOR_PROGRESS_OWNER_ID, CoordinatorRunReport, InvestigationCoordinatorRunnerPort,
    InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
    RunCoordinatorInput,
};
use rig::completion::{Prompt, Usage};
use rig::{client::ProviderClient, providers::openai};

use super::investigation_agents::{
    BuildCoordinatorAgentInput, build_coordinator_agent, build_coordinator_prompt,
};
use super::llm_usage_mapper::{MapRigUsageToSnapshotInput, map_rig_usage_to_llm_usage_snapshot};
use super::request_count_hook::RequestCountHook;

const COORDINATOR_MAX_TURNS: usize = 20;
const COORDINATOR_TOOL_CONCURRENCY: usize = 8;

pub struct OpenAiInvestigationCoordinatorRunnerInput {
    pub openai_api_key: String,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub language: String,
}

pub struct OpenAiInvestigationCoordinatorRunner {
    openai_api_key: String,
    datadog_site: String,
    github_scope_org: String,
    language: String,
}

impl OpenAiInvestigationCoordinatorRunner {
    pub fn new(input: OpenAiInvestigationCoordinatorRunnerInput) -> Self {
        Self {
            openai_api_key: input.openai_api_key,
            datadog_site: input.datadog_site,
            github_scope_org: input.github_scope_org,
            language: input.language,
        }
    }
}

#[async_trait]
impl InvestigationCoordinatorRunnerPort for OpenAiInvestigationCoordinatorRunner {
    async fn run(
        &self,
        input: RunCoordinatorInput,
    ) -> Result<CoordinatorRunReport, AgentRunFailedError> {
        let request_count_hook = RequestCountHook::new();
        let coordinator_prompt = build_coordinator_prompt(&input.alert_context);
        let openai_client = openai::Client::from_val(self.openai_api_key.clone().into());
        let coordinator_agent = build_coordinator_agent(BuildCoordinatorAgentInput {
            client: openai_client,
            resources: Arc::new(input.context.resources),
            datadog_site: self.datadog_site.clone(),
            github_scope_org: self.github_scope_org.clone(),
            runtime: input.context.runtime,
            on_progress_event: Arc::clone(&input.on_progress_event),
            language: self.language.clone(),
        });

        let prompt_response = coordinator_agent
            .prompt(coordinator_prompt)
            .max_turns(COORDINATOR_MAX_TURNS)
            .with_tool_concurrency(COORDINATOR_TOOL_CONCURRENCY)
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
        let usage = Some(prompt_response.total_usage);

        publish_message_output_created_event(PublishMessageOutputCreatedEventInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
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
        })
    }
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
