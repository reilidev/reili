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
    llm_provider_settings::LlmProviderSettings, llm_usage_collector::LlmUsageCollector,
    progress_reporting_sub_agent_tool::ProgressReportingSubAgentTool,
};
use instructions::{BuildTaskInstructionsInput, build_task_instructions};
use specialists::{
    BuildDatadogAgentInput, BuildGithubAgentInput, CreateSpecialistAgentFactoryInput,
    SpecialistAgentConfig, SpecialistAgentFactory,
};

pub(super) const DATADOG_PROGRESS_OWNER_ID: &str = "investigate_datadog";
pub(super) const GITHUB_PROGRESS_OWNER_ID: &str = "investigate_github";

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
}

impl<C> TaskAgentFactory<C>
where
    C: CompletionClient,
{
    #[must_use]
    pub fn new(input: CreateTaskAgentFactoryInput<C>) -> Self {
        Self {
            client: input.client,
            config: input.config,
        }
    }
}

impl<C> TaskAgentFactory<C>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    #[must_use]
    pub fn build(&self, input: BuildTaskAgentInput) -> CompletionAgent<C> {
        let specialist_factory = SpecialistAgentFactory::new(CreateSpecialistAgentFactoryInput {
            client: self.client.clone(),
            config: self.specialist_config(),
        });
        let datadog_agent = specialist_factory.build_datadog(BuildDatadogAgentInput {
            run_context: &input.run_context,
            toolset: input.toolsets.datadog.clone(),
        });
        let github_agent = specialist_factory.build_github(BuildGithubAgentInput {
            run_context: &input.run_context,
            toolset: input.toolsets.github.clone(),
            github_scope_org: self.config.instructions.github_scope_org.clone(),
        });

        let builder = self
            .client
            .clone()
            .agent(self.config.settings.task_runner_model.clone())
            .name("TaskRunner")
            .preamble(&build_task_instructions(BuildTaskInstructionsInput {
                datadog_site: self.config.instructions.datadog_site.clone(),
                github_scope_org: self.config.instructions.github_scope_org.clone(),
                runtime: input.run_context.execution.runtime.clone(),
                language: self.config.instructions.language.clone(),
                additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
            }))
            .default_max_turns(self.config.settings.task_runner_max_turns)
            .additional_params(self.config.settings.additional_params.clone());
        let builder = with_max_tokens(builder, self.config.settings.max_tokens);

        builder
            .tools(input.toolsets.datadog.lead_tools())
            .tool(ReportProgressTool::new(ReportProgressToolInput {
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
            }))
            .tool(ProgressReportingSubAgentTool::new(
                datadog_agent,
                DATADOG_PROGRESS_OWNER_ID.to_string(),
                Arc::clone(&input.run_context.execution.on_progress_event),
            ))
            .tool(SearchSlackMessagesTool::new(
                Arc::clone(&input.run_context.resources.slack_message_search_port),
                input.run_context.slack_action_token.clone(),
            ))
            .tool(SearchWebTool::new(Arc::clone(&input.run_context.resources)))
            .tool(ProgressReportingSubAgentTool::new(
                github_agent,
                GITHUB_PROGRESS_OWNER_ID.to_string(),
                Arc::clone(&input.run_context.execution.on_progress_event),
            ))
            .build()
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
