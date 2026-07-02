use std::sync::Arc;

use reili_core::task::TaskResources;
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::spawn::{
    ComposeSpawnedPreambleInput, ResolveSpawnSelectionInput, compose_spawned_sub_agent_preamble,
    resolve_spawn_selection,
};
use super::{TaskAgentExecutionContext, TaskAgentRunContext, with_max_tokens};
use crate::outbound::agents::connector::PreparedConnector;
use crate::outbound::agents::runner::execution_hook::AgentExecutionHook;
use crate::outbound::agents::runner::provider_settings::LlmProviderSettings;
use crate::outbound::agents::tools::{
    ReportProgressTool, ReportProgressToolInput, SpawnedSubAgentSpec,
};

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

pub(super) struct BuildSpawnedSubAgentInput {
    pub(super) run_context: TaskAgentRunContext,
    pub(super) prepared_connectors: Vec<Arc<dyn PreparedConnector>>,
    pub(super) spec: SpawnedSubAgentSpec,
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
    /// Build a sub-agent from a lead-composed spawn spec: resolve the selected tools, compose
    /// the fixed-frame preamble around the lead-generated mission, and attach the shared
    /// execution hook and progress tooling.
    pub(super) fn build_spawned_sub_agent(&self, input: BuildSpawnedSubAgentInput) -> SubAgent<C> {
        let common = self.common_input(&input.run_context, input.spec.owner_id.clone());
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

        let selection = resolve_spawn_selection(&ResolveSpawnSelectionInput {
            prepared_connectors: &input.prepared_connectors,
            resources: &resources,
            tool_names: &input.spec.tool_names,
        });
        let preamble = compose_spawned_sub_agent_preamble(&ComposeSpawnedPreambleInput {
            language: &language,
            additional_system_prompt: additional_system_prompt.as_deref(),
            lead_instructions: &input.spec.instructions,
            guardrails: &selection.guardrails,
        });

        let builder = client
            .agent(settings.sub_agent_model.clone())
            .name(&input.spec.name)
            .description("Dynamically spawned sub-agent")
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
            .tools(selection.tools)
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
