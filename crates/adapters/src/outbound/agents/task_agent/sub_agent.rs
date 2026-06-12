use std::sync::Arc;

use reili_core::task::TaskResources;
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::{TaskAgentExecutionContext, TaskAgentRunContext, with_max_tokens};
use crate::outbound::agents::connector::{PreparedConnector, SubAgentPromptContext};
use crate::outbound::agents::runner::execution_hook::AgentExecutionHook;
use crate::outbound::agents::runner::provider_settings::LlmProviderSettings;
use crate::outbound::agents::tools::{ReportProgressTool, ReportProgressToolInput, SearchWebTool};

type SubAgent<C> = Agent<<C as CompletionClient>::CompletionModel, AgentExecutionHook>;

#[derive(Clone)]
pub(super) struct SubAgentFactory<C>
where
    C: CompletionClient,
{
    client: C,
    config: SubAgentConfig,
}

pub(super) struct CreateSubAgentFactoryInput<C>
where
    C: CompletionClient,
{
    pub(super) client: C,
    pub(super) config: SubAgentConfig,
}

#[derive(Clone)]
pub(super) struct SubAgentConfig {
    pub(super) settings: LlmProviderSettings,
    pub(super) language: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) struct BuildSubAgentInput {
    pub(super) run_context: TaskAgentRunContext,
    pub(super) prepared: Arc<dyn PreparedConnector>,
    pub(super) owner_id: String,
}

struct SubAgentCommonInput<C>
where
    C: CompletionClient,
{
    client: C,
    config: SubAgentConfig,
    resources: Arc<TaskResources>,
    execution: TaskAgentExecutionContext,
    owner_id: String,
}

impl<C> SubAgentFactory<C>
where
    C: CompletionClient,
{
    pub(super) fn new(input: CreateSubAgentFactoryInput<C>) -> Self {
        Self {
            client: input.client,
            config: input.config,
        }
    }
}

impl<C> SubAgentFactory<C>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    /// Build a sub-agent from a prepared connector.
    pub(super) fn build_sub_agent(&self, input: BuildSubAgentInput) -> SubAgent<C> {
        let common = self.common_input(&input.run_context, input.owner_id);
        let SubAgentCommonInput {
            client,
            config,
            resources,
            execution,
            owner_id,
        } = common;
        let SubAgentConfig {
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

        let prompt_context = SubAgentPromptContext {
            language,
            additional_system_prompt,
        };
        let preamble = input.prepared.sub_agent_preamble(&prompt_context);
        let descriptor = input.prepared.descriptor();
        let agent_name = descriptor.agent_name.clone();
        let agent_description = descriptor.agent_description.clone();

        let builder = client
            .agent(settings.sub_agent_model.clone())
            .name(&agent_name)
            .description(&agent_description)
            .preamble(&preamble)
            .default_max_turns(settings.sub_agent_max_turns)
            .additional_params(settings.additional_params.clone());
        let builder = with_max_tokens(builder, settings.sub_agent_max_tokens);

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
            .tools(input.prepared.sub_agent_tools())
            .tool(SearchWebTool::new(resources))
            .build()
    }

    fn common_input(
        &self,
        run_context: &TaskAgentRunContext,
        owner_id: String,
    ) -> SubAgentCommonInput<C> {
        SubAgentCommonInput {
            client: self.client.clone(),
            config: self.config.clone(),
            resources: Arc::clone(&run_context.resources),
            execution: run_context.execution.clone(),
            owner_id,
        }
    }
}
