use std::sync::Arc;

use reili_core::task::TaskResources;
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::instructions::{
    BuildDatadogInstructionsInput, BuildEsaInstructionsInput, BuildGithubInstructionsInput,
    build_datadog_instructions, build_esa_instructions, build_github_instructions,
};
use super::{
    DATADOG_PROGRESS_OWNER_ID, ESA_PROGRESS_OWNER_ID, GITHUB_PROGRESS_OWNER_ID,
    TaskAgentExecutionContext, TaskAgentRunContext, with_max_tokens,
};
use crate::outbound::agents::agent_execution_hook::AgentExecutionHook;
use crate::outbound::agents::datadog_mcp_tools::DatadogMcpToolset;
use crate::outbound::agents::github_mcp_tools::GitHubMcpToolset;
use crate::outbound::agents::llm_provider_settings::LlmProviderSettings;
use crate::outbound::agents::tools::{
    ReportProgressTool, ReportProgressToolInput, SearchPostsTool, SearchWebTool,
};
use crate::outbound::esa::EsaPostSearchPort;

type SpecialistAgent<C> = Agent<<C as CompletionClient>::CompletionModel, AgentExecutionHook>;

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

pub(super) struct BuildDatadogAgentInput<'a> {
    pub(super) run_context: &'a TaskAgentRunContext,
    pub(super) toolset: DatadogMcpToolset,
}

pub(super) struct BuildGithubAgentInput<'a> {
    pub(super) run_context: &'a TaskAgentRunContext,
    pub(super) toolset: GitHubMcpToolset,
    pub(super) github_scope_org: String,
}

pub(super) struct BuildEsaAgentInput<'a> {
    pub(super) run_context: &'a TaskAgentRunContext,
    pub(super) esa_post_search_port: Arc<dyn EsaPostSearchPort>,
    pub(super) esa_team_name: String,
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
    pub(super) fn build_datadog(&self, input: BuildDatadogAgentInput<'_>) -> SpecialistAgent<C> {
        let common = self.common_input(input.run_context, DATADOG_PROGRESS_OWNER_ID);
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

        let builder = client
            .agent(settings.specialist_model.clone())
            .name("investigate_datadog")
            .description(
                "Delegates Datadog observability and security investigation tasks.
When instructing this specialist, include the relevant background, context, and why the investigation matters, not just the immediate question.",
            )
            .preamble(&build_datadog_instructions(BuildDatadogInstructionsInput {
                language,
                additional_system_prompt,
            }))
            .default_max_turns(settings.specialist_max_turns)
            .additional_params(settings.additional_params.clone());
        let builder = with_max_tokens(builder, settings.max_tokens);

        let builder = builder
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
            .tools(input.toolset.specialist_tools())
            .tool(SearchWebTool::new(resources));

        builder.build()
    }

    pub(super) fn build_github(&self, input: BuildGithubAgentInput<'_>) -> SpecialistAgent<C> {
        let common = self.common_input(input.run_context, GITHUB_PROGRESS_OWNER_ID);
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

        let builder = client
            .agent(settings.specialist_model.clone())
            .name("investigate_github")
            .description(
                "Delegates GitHub repository, code, pull request, Actions, and Dependabot investigation tasks.
When instructing this specialist, include the relevant background, context, and why the investigation matters, not just the immediate question.",
            )
            .preamble(&build_github_instructions(BuildGithubInstructionsInput {
                language,
                github_scope_org: input.github_scope_org,
                additional_system_prompt,
            }))
            .default_max_turns(settings.specialist_max_turns)
            .additional_params(settings.additional_params.clone());
        let builder = with_max_tokens(builder, settings.max_tokens);
        let builder = builder
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
            .tools(input.toolset.specialist_tools())
            .tool(SearchWebTool::new(resources));

        builder.build()
    }

    pub(super) fn build_esa(&self, input: BuildEsaAgentInput<'_>) -> SpecialistAgent<C> {
        let common = self.common_input(input.run_context, ESA_PROGRESS_OWNER_ID);
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

        let builder = client
            .agent(settings.specialist_model.clone())
            .name("investigate_esa")
            .description(
                "Delegates esa internal documentation, runbook, design note, team knowledge, and broader knowledge base search tasks.
When instructing this specialist, include the relevant background, context, and why the documentation search matters, not just the immediate keywords.",
            )
            .preamble(&build_esa_instructions(BuildEsaInstructionsInput {
                language,
                team_name: input.esa_team_name,
                additional_system_prompt,
            }))
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
            .tool(SearchPostsTool::new(input.esa_post_search_port))
            .tool(SearchWebTool::new(resources))
            .build()
    }

    fn common_input(
        &self,
        run_context: &TaskAgentRunContext,
        owner_id: &'static str,
    ) -> SpecialistAgentCommonInput<C> {
        SpecialistAgentCommonInput {
            client: self.client.clone(),
            config: self.config.clone(),
            resources: Arc::clone(&run_context.resources),
            execution: run_context.execution.clone(),
            owner_id: owner_id.to_string(),
        }
    }
}
