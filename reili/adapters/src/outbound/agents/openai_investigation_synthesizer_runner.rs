use std::sync::Arc;

use async_trait::async_trait;
use rig::completion::{Prompt, Usage};
use rig::{client::ProviderClient, providers::openai};
use reili_shared::errors::{AgentRole, AgentRunFailedError};
use reili_shared::ports::outbound::{
    InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
    InvestigationSynthesizerRunnerPort, RunSynthesizerInput, SYNTHESIZER_PROGRESS_OWNER_ID,
    SynthesizerRunReport,
};

use super::investigation_agents::{
    BuildSynthesizerAgentInput, build_synthesizer_agent, build_synthesizer_prompt,
};
use super::llm_usage_mapper::{MapRigUsageToSnapshotInput, map_rig_usage_to_llm_usage_snapshot};
use super::request_count_hook::RequestCountHook;

const SYNTHESIZER_MAX_TURNS: usize = 1;
const FALLBACK_SYNTHESIS_REPORT: &str = "Investigation completed but failed to generate a report.";

pub struct OpenAiInvestigationSynthesizerRunnerInput {
    pub openai_api_key: String,
    pub language: String,
}

pub struct OpenAiInvestigationSynthesizerRunner {
    openai_api_key: String,
    language: String,
}

impl OpenAiInvestigationSynthesizerRunner {
    pub fn new(input: OpenAiInvestigationSynthesizerRunnerInput) -> Self {
        Self {
            openai_api_key: input.openai_api_key,
            language: input.language,
        }
    }
}

#[async_trait]
impl InvestigationSynthesizerRunnerPort for OpenAiInvestigationSynthesizerRunner {
    async fn run(
        &self,
        input: RunSynthesizerInput,
    ) -> Result<SynthesizerRunReport, AgentRunFailedError> {
        let request_count_hook = RequestCountHook::new();
        let synthesizer_prompt = build_synthesizer_prompt(&input.result, &input.alert_context);
        let openai_client = openai::Client::from_val(self.openai_api_key.clone().into());
        let synthesizer_agent = build_synthesizer_agent(BuildSynthesizerAgentInput {
            client: openai_client,
            language: self.language.clone(),
        });

        let prompt_response = synthesizer_agent
            .prompt(synthesizer_prompt)
            .max_turns(SYNTHESIZER_MAX_TURNS)
            .with_hook(request_count_hook.clone())
            .extended_details()
            .await
            .map_err(|error| {
                create_failed_error(CreateSynthesizerRunnerFailedErrorInput {
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

        let report_text = if prompt_response.output.is_empty() {
            FALLBACK_SYNTHESIS_REPORT.to_string()
        } else {
            prompt_response.output
        };

        Ok(SynthesizerRunReport {
            report_text,
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
            owner_id: SYNTHESIZER_PROGRESS_OWNER_ID.to_string(),
            event: InvestigationProgressEvent::MessageOutputCreated,
        })
        .await
        .map_err(|error| {
            create_failed_error(CreateSynthesizerRunnerFailedErrorInput {
                usage: input.usage,
                requests: input.requests,
                cause_message: format!("Failed to publish progress event: {error}"),
            })
        })?;

    Ok(())
}

struct CreateSynthesizerRunnerFailedErrorInput {
    usage: Option<Usage>,
    requests: u32,
    cause_message: String,
}

fn create_failed_error(input: CreateSynthesizerRunnerFailedErrorInput) -> AgentRunFailedError {
    let usage = map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
        usage: input.usage,
        requests: input.requests,
    });

    AgentRunFailedError::new(AgentRole::Synthesizer, usage, input.cause_message)
}
