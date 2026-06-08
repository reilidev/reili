use std::sync::Arc;

use reili_core::task::TaskResources;
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::{TaskAgentExecutionContext, TaskAgentRunContext, with_max_tokens};
use crate::outbound::agents::connector::{PreparedConnector, SpecialistPromptContext};
use crate::outbound::agents::runner::execution_hook::AgentExecutionHook;
use crate::outbound::agents::runner::provider_settings::LlmProviderSettings;
use crate::outbound::agents::tools::{ReportProgressTool, ReportProgressToolInput, SearchWebTool};

type SpecialistAgent<C> = Agent<<C as CompletionClient>::CompletionModel, AgentExecutionHook>;

#[derive(Clone)]
pub(super) struct SpecialistAgentFactory<C>
where
    C: CompletionClient,
{
    client: C,
    config: SpecialistAgentConfig,
}

pub(super) struct CreateSpecialistAgentFactoryInput<C>
where
    C: CompletionClient,
{
    pub(super) client: C,
    pub(super) config: SpecialistAgentConfig,
}

#[derive(Clone)]
pub(super) struct SpecialistAgentConfig {
    pub(super) settings: LlmProviderSettings,
    pub(super) language: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) struct BuildSpecialistAgentInput {
    pub(super) run_context: TaskAgentRunContext,
    pub(super) prepared: Arc<dyn PreparedConnector>,
    pub(super) owner_id: String,
}

struct SpecialistAgentCommonInput<C>
where
    C: CompletionClient,
{
    client: C,
    config: SpecialistAgentConfig,
    resources: Arc<TaskResources>,
    execution: TaskAgentExecutionContext,
    owner_id: String,
}

impl<C> SpecialistAgentFactory<C>
where
    C: CompletionClient,
{
    pub(super) fn new(input: CreateSpecialistAgentFactoryInput<C>) -> Self {
        Self {
            client: input.client,
            config: input.config,
        }
    }
}

impl<C> SpecialistAgentFactory<C>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    /// Build a specialist agent from a prepared connector.
    pub(super) fn build_specialist(&self, input: BuildSpecialistAgentInput) -> SpecialistAgent<C> {
        let common = self.common_input(&input.run_context, input.owner_id);
        let SpecialistAgentCommonInput {
            client,
            config,
            resources,
            execution,
            owner_id,
        } = common;
        let SpecialistAgentConfig {
            settings,
            language,
            additional_system_prompt,
        } = config;
        let TaskAgentExecutionContext {
            logger,
            runtime,
            cancellation,
            on_progress_event,
            usage_collector,
        } = execution;

        let prompt_context = SpecialistPromptContext {
            language,
            additional_system_prompt,
        };
        let preamble = input.prepared.specialist_preamble(&prompt_context);
        let descriptor = input.prepared.descriptor();
        let agent_name = descriptor.agent_name.clone();
        let agent_description = descriptor.agent_description.clone();

        let builder = client
            .agent(settings.specialist_model.clone())
            .name(&agent_name)
            .description(&agent_description)
            .preamble(&preamble)
            .default_max_turns(settings.specialist_max_turns)
            .additional_params(settings.additional_params.clone());
        let builder = with_max_tokens(builder, settings.max_tokens);

        builder
            .hook(AgentExecutionHook::new(
                owner_id.clone(),
                runtime,
                cancellation,
                logger,
                Arc::clone(&on_progress_event),
                usage_collector,
            ))
            .tool(ReportProgressTool::new(ReportProgressToolInput {
                on_progress_event: Arc::clone(&on_progress_event),
                owner_id,
            }))
            .tools(input.prepared.specialist_tools())
            .tool(SearchWebTool::new(resources))
            .build()
    }

    fn common_input(
        &self,
        run_context: &TaskAgentRunContext,
        owner_id: String,
    ) -> SpecialistAgentCommonInput<C> {
        SpecialistAgentCommonInput {
            client: self.client.clone(),
            config: self.config.clone(),
            resources: Arc::clone(&run_context.resources),
            execution: run_context.execution.clone(),
            owner_id,
        }
    }
}
