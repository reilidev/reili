use std::sync::Arc;

use reili_core::logger::Logger;
use reili_core::secret::SecretString;
use reili_core::task::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskCancellation, TaskProgressEventPort, TaskResources,
    TaskRuntime,
};
use rig::agent::Agent;
use rig::prelude::CompletionClient;

mod instructions;
mod prompt;
mod specialists;

pub use prompt::build_task_prompt;

use super::datadog_mcp_tools::DatadogMcpToolset;
use super::github_mcp_tools::GitHubMcpToolset;
use super::tools::{
    ReportProgressTool, ReportProgressToolInput, SearchSlackMessagesTool, SearchWebTool,
};
use super::{
    llm_provider_settings::LlmProviderSettings,
    llm_usage_collector::LlmUsageCollector,
    progress_reporting_sub_agent_tool::{
        ProgressReportingSubAgentTool, ProgressReportingSubAgentToolInput,
    },
};
use crate::outbound::esa::EsaPostSearchPort;
use instructions::{BuildTaskInstructionsInput, build_task_instructions};
use specialists::{
    BuildDatadogAgentInput, BuildEsaAgentInput, BuildGithubAgentInput,
    CreateSpecialistAgentFactoryInput, DATADOG_AGENT_DESCRIPTION, DATADOG_AGENT_NAME,
    ESA_AGENT_DESCRIPTION, ESA_AGENT_NAME, GITHUB_AGENT_DESCRIPTION, GITHUB_AGENT_NAME,
    SpecialistAgentConfig, SpecialistAgentFactory,
};

pub(super) const DATADOG_PROGRESS_OWNER_ID: &str = "investigate_datadog";
pub(super) const GITHUB_PROGRESS_OWNER_ID: &str = "investigate_github";
pub(super) const ESA_PROGRESS_OWNER_ID: &str = "investigate_esa";

type CompletionAgent<C> = Agent<<C as CompletionClient>::CompletionModel>;

pub struct TaskAgentFactory<C>
where
    C: CompletionClient,
{
    client: C,
    config: TaskAgentConfig,
}

pub struct CreateTaskAgentFactoryInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub config: TaskAgentConfig,
}

pub struct BuildTaskAgentInput {
    pub run_context: TaskAgentRunContext,
    pub toolsets: TaskAgentToolsets,
    pub connectors: TaskAgentConnectors,
}

impl<C> TaskAgentFactory<C>
where
    C: CompletionClient,
{
    pub fn new(input: CreateTaskAgentFactoryInput<C>) -> Self {
        Self {
            client: input.client,
            config: input.config,
        }
    }
}

impl<C> TaskAgentFactory<C>
where
    C: CompletionClient + Clone + Send + Sync + 'static,
    C::CompletionModel: 'static,
{
    #[must_use]
    pub fn build(&self, input: BuildTaskAgentInput) -> CompletionAgent<C> {
        let specialist_factory = SpecialistAgentFactory::new(CreateSpecialistAgentFactoryInput {
            client: self.client.clone(),
            config: self.specialist_config(),
        });
        let esa_team_name = input
            .connectors
            .esa
            .as_ref()
            .map(|connector| connector.team_name.clone());
        let memory_context_section = build_memory_context_section(&input.run_context.memory_items);
        let datadog_agent_factory = {
            let specialist_factory = specialist_factory.clone();
            let run_context = input.run_context.clone();
            let toolset = input.toolsets.datadog.clone();
            Arc::new(move |owner_id| {
                specialist_factory.build_datadog(BuildDatadogAgentInput {
                    run_context: &run_context,
                    toolset: toolset.clone(),
                    owner_id,
                })
            })
        };
        let github_agent_factory = {
            let specialist_factory = specialist_factory.clone();
            let run_context = input.run_context.clone();
            let toolset = input.toolsets.github.clone();
            let github_scope_org = self.config.instructions.github_scope_org.clone();
            Arc::new(move |owner_id| {
                specialist_factory.build_github(BuildGithubAgentInput {
                    run_context: &run_context,
                    toolset: toolset.clone(),
                    github_scope_org: github_scope_org.clone(),
                    owner_id,
                })
            })
        };
        let esa_agent_tool = input.connectors.esa.as_ref().map(|connector| {
            let specialist_factory = specialist_factory.clone();
            let run_context = input.run_context.clone();
            let post_search_port = Arc::clone(&connector.post_search_port);
            let team_name = connector.team_name.clone();
            let agent_factory = Arc::new(move |owner_id| {
                specialist_factory.build_esa(BuildEsaAgentInput {
                    run_context: &run_context,
                    esa_post_search_port: Arc::clone(&post_search_port),
                    esa_team_name: team_name.clone(),
                    owner_id,
                })
            });

            ProgressReportingSubAgentTool::new(ProgressReportingSubAgentToolInput {
                agent_factory,
                agent_name: ESA_AGENT_NAME.to_string(),
                agent_description: Some(ESA_AGENT_DESCRIPTION.to_string()),
                owner_id: ESA_PROGRESS_OWNER_ID.to_string(),
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                tool_concurrency: self.config.settings.tool_concurrency,
                shared_prompt_context: memory_context_section.clone(),
            })
        });

        let builder = self
            .client
            .clone()
            .agent(self.config.settings.task_runner_model.clone())
            .name("TaskRunner")
            .preamble(&build_task_instructions(BuildTaskInstructionsInput {
                datadog_site: self.config.instructions.datadog_site.clone(),
                github_scope_org: self.config.instructions.github_scope_org.clone(),
                esa_team_name,
                runtime: input.run_context.execution.runtime.clone(),
                language: self.config.instructions.language.clone(),
                additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
            }))
            .default_max_turns(self.config.settings.task_runner_max_turns)
            .additional_params(self.config.settings.additional_params.clone());
        let builder = with_max_tokens(builder, self.config.settings.max_tokens);

        let builder = builder
            .tools(input.toolsets.datadog.lead_tools())
            .tool(ReportProgressTool::new(ReportProgressToolInput {
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
            }))
            .tool(ProgressReportingSubAgentTool::new(
                ProgressReportingSubAgentToolInput {
                    agent_factory: datadog_agent_factory,
                    agent_name: DATADOG_AGENT_NAME.to_string(),
                    agent_description: Some(DATADOG_AGENT_DESCRIPTION.to_string()),
                    owner_id: DATADOG_PROGRESS_OWNER_ID.to_string(),
                    on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                    tool_concurrency: self.config.settings.tool_concurrency,
                    shared_prompt_context: memory_context_section.clone(),
                },
            ))
            .tool(SearchSlackMessagesTool::new(
                Arc::clone(&input.run_context.resources.slack_message_search_port),
                input.run_context.slack_action_token.clone(),
            ))
            .tool(SearchWebTool::new(Arc::clone(&input.run_context.resources)))
            .tool(ProgressReportingSubAgentTool::new(
                ProgressReportingSubAgentToolInput {
                    agent_factory: github_agent_factory,
                    agent_name: GITHUB_AGENT_NAME.to_string(),
                    agent_description: Some(GITHUB_AGENT_DESCRIPTION.to_string()),
                    owner_id: GITHUB_PROGRESS_OWNER_ID.to_string(),
                    on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                    tool_concurrency: self.config.settings.tool_concurrency,
                    shared_prompt_context: memory_context_section.clone(),
                },
            ));
        let builder = match esa_agent_tool {
            Some(tool) => builder.tool(tool),
            None => builder,
        };

        builder.build()
    }

    fn specialist_config(&self) -> SpecialistAgentConfig {
        SpecialistAgentConfig {
            settings: self.config.settings.clone(),
            language: self.config.instructions.language.clone(),
            additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TaskAgentConfig {
    pub settings: LlmProviderSettings,
    pub instructions: AgentInstructionsConfig,
}

#[derive(Clone)]
pub struct AgentInstructionsConfig {
    pub datadog_site: String,
    pub github_scope_org: String,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

#[derive(Clone)]
pub struct TaskAgentRunContext {
    pub resources: Arc<TaskResources>,
    pub execution: TaskAgentExecutionContext,
    pub slack_action_token: Option<SecretString>,
    pub memory_items: Vec<reili_core::task::TaskMemoryItem>,
}

#[derive(Clone)]
pub struct TaskAgentExecutionContext {
    pub logger: Arc<dyn Logger>,
    pub runtime: TaskRuntime,
    pub cancellation: TaskCancellation,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub usage_collector: LlmUsageCollector,
}

#[derive(Clone)]
pub struct TaskAgentToolsets {
    pub datadog: DatadogMcpToolset,
    pub github: GitHubMcpToolset,
}

#[derive(Clone)]
pub struct TaskAgentConnectors {
    // Datadog and GitHub should move here when their specialist dependencies are modeled as
    // connector ports instead of MCP toolsets.
    pub esa: Option<TaskAgentEsaConnector>,
}

#[derive(Clone)]
pub struct TaskAgentEsaConnector {
    pub team_name: String,
    pub post_search_port: Arc<dyn EsaPostSearchPort>,
}

fn with_max_tokens<M, H>(
    builder: rig::agent::AgentBuilder<M, H>,
    max_tokens: Option<u64>,
) -> rig::agent::AgentBuilder<M, H>
where
    M: rig::completion::CompletionModel,
    H: rig::agent::PromptHook<M>,
{
    match max_tokens {
        Some(value) => builder.max_tokens(value),
        None => builder,
    }
}

fn build_memory_context_section(
    memory_items: &[reili_core::task::TaskMemoryItem],
) -> Option<String> {
    if memory_items.is_empty() {
        return None;
    }

    Some(format!(
        "# Memory Context\n{}",
        prompt::build_memory_context(memory_items)
    ))
}
