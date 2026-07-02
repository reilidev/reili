use std::sync::Arc;

use reili_core::logger::Logger;
use reili_core::secret::SecretString;
use reili_core::task::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskCancellation, TaskProgressEventPort, TaskResources,
    TaskRuntime,
};
use rig::agent::Agent;
use rig::prelude::CompletionClient;
use rig::tool::ToolDyn;

mod instructions;
mod prompt;
mod spawn;
mod sub_agent;

pub use prompt::{BuildTaskPromptInput, build_task_prompt};

use super::connector::PreparedConnector;
use super::runner::provider_settings::LlmProviderSettings;
use super::runner::usage_collector::LlmUsageCollector;
use super::tools::{
    ReportProgressTool, ReportProgressToolInput, SearchSlackMessagesTool, SearchWebTool,
    SpawnAgentTool, SpawnAgentToolInput, SpawnedSubAgentSpec,
};
use instructions::{BuildTaskInstructionsInput, build_task_instructions};
use spawn::{render_spawn_tool_catalog, spawn_catalog_tool_names, spawn_tool_catalog_groups};
use sub_agent::{
    BuildSpawnedSubAgentInput, CreateSubAgentFactoryInput, SubAgentConfig, SubAgentFactory,
};

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
    pub prepared_connectors: Vec<Arc<dyn PreparedConnector>>,
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
        let sub_agent_factory = SubAgentFactory::new(CreateSubAgentFactoryInput {
            client: self.client.clone(),
            config: self.sub_agent_config(),
        });
        let memory_context_section = build_memory_context_section(&input.run_context.memory_items);
        let catalog_groups = spawn_tool_catalog_groups(&input.prepared_connectors);

        let builder = self
            .client
            .clone()
            .agent(self.config.settings.task_runner_model.clone())
            .name("TaskRunner")
            .preamble(&build_task_instructions(BuildTaskInstructionsInput {
                additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
                spawn_tool_catalog: render_spawn_tool_catalog(&catalog_groups),
            }))
            .default_max_turns(self.config.settings.task_runner_max_turns)
            .additional_params(self.config.settings.additional_params.clone());
        let builder = with_max_tokens(builder, self.config.settings.task_runner_max_tokens);

        let lead_tools: Vec<Box<dyn ToolDyn>> = input
            .prepared_connectors
            .iter()
            .flat_map(|prepared| prepared.lead_tools())
            .collect();

        // spawn_agent replaces per-connector sub-agent tools: the lead composes each
        // sub-agent's instructions and tool selection per delegation.
        let agent_factory = {
            let sub_agent_factory = sub_agent_factory.clone();
            let run_context = input.run_context.clone();
            let prepared_connectors = input.prepared_connectors.clone();
            Arc::new(move |spec: SpawnedSubAgentSpec| {
                sub_agent_factory.build_spawned_sub_agent(BuildSpawnedSubAgentInput {
                    run_context: run_context.clone(),
                    prepared_connectors: prepared_connectors.clone(),
                    spec,
                })
            })
        };

        builder
            .tools(lead_tools)
            .tool(ReportProgressTool::new(ReportProgressToolInput {
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
            }))
            .tool(SearchSlackMessagesTool::new(
                Arc::clone(&input.run_context.resources.slack_message_search_port),
                input.run_context.slack_action_token.clone(),
            ))
            .tool(SearchWebTool::new(Arc::clone(&input.run_context.resources)))
            .tool(SpawnAgentTool::new(SpawnAgentToolInput {
                agent_factory,
                available_tool_names: spawn_catalog_tool_names(&catalog_groups),
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                tool_concurrency: self.config.settings.tool_concurrency,
                shared_prompt_context: memory_context_section,
            }))
            .build()
    }

    fn sub_agent_config(&self) -> SubAgentConfig {
        SubAgentConfig {
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
