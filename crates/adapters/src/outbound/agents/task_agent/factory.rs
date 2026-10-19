use std::sync::Arc;

use reili_core::cycle_agent::{ReportSlot, RunBudget};
use reili_core::logger::Logger;
use reili_core::secret::SecretString;
use reili_core::task::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskCancellation, TaskProgressEventPort, TaskResources,
    TaskRuntime,
};
use rig::agent::{Agent, AgentBuilder, WithBuilderTools};
use rig::prelude::{AgentClientExt, CompletionClient};

use super::super::connector::{PreparedConnector, ToolCatalogGroup};
use super::super::runner::execution_hook::AgentExecutionHook;
use super::super::runner::provider_settings::LlmProviderSettings;
use super::super::runner::usage_collector::LlmUsageCollector;
use super::super::tools::{
    ReportProgressTool, ReportProgressToolInput, SaveMemoryTool, SaveMemoryToolInput,
    SaveSharedMemoryTool, SearchSlackMessagesTool, SearchWebTool, SpawnAgentTool,
    SpawnAgentToolInput, SpawnedSubAgentSpec, SubmitCycleReportTool,
};
use super::instructions::{BuildTaskInstructionsInput, build_task_instructions};
use super::objective_instructions::{
    BuildObjectiveInstructionsInput, build_objective_instructions,
};
use super::spawn::{
    render_spawn_tool_catalog, spawn_catalog_tool_names, spawn_tool_catalog_groups,
};
use super::sub_agent::{
    BuildSpawnedSubAgentInput, CreateSubAgentFactoryInput, SubAgentConfig, SubAgentFactory,
};

/// Tools an objective cycle's agents run at once, lead and sub-agents alike.
pub(crate) const OBJECTIVE_TOOL_CONCURRENCY: usize = 6;

pub struct TaskAgentFactory<C>
where
    C: CompletionClient,
{
    client: C,
    sub_agent_client: C,
    config: TaskAgentConfig,
}

pub struct CreateTaskAgentFactoryInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub sub_agent_client: C,
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
            sub_agent_client: input.sub_agent_client,
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
    pub fn build(&self, input: BuildTaskAgentInput) -> Agent {
        let run_context = &input.run_context;
        let catalog_groups =
            spawn_tool_catalog_groups(&input.prepared_connectors, has_web_search(run_context));
        let preamble = build_task_instructions(BuildTaskInstructionsInput {
            additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
            spawn_tool_catalog: render_spawn_tool_catalog(&catalog_groups),
        });
        let builder = self
            .lead_agent_builder(LeadAgentInput {
                name: "TaskRunner",
                preamble: &preamble,
                run_context,
                prepared_connectors: &input.prepared_connectors,
                catalog_groups: &catalog_groups,
                tool_concurrency: self.config.settings.tool_concurrency,
                shared_prompt_context: build_memory_context_section(&run_context.memory_items),
            })
            .tool(SearchSlackMessagesTool::new(
                Arc::clone(&run_context.resources.slack_message_search_port),
                run_context.slack_action_token.clone(),
            ));

        // save_memory / save_shared_memory are lead-only tools; sub-agents surface facts for the
        // lead to persist. Both are registered only when a Canvas memory backend is configured.
        let builder = match &run_context.resources.canvas_memory_port {
            Some(canvas_memory_port) => {
                let save_memory = SaveMemoryToolInput {
                    canvas_memory_port: Arc::clone(canvas_memory_port),
                    channel_id: run_context.execution.runtime.channel.clone(),
                    channel_name: None,
                    source_message_ts: run_context.execution.runtime.thread_ts.clone(),
                };
                builder
                    .tool(SaveMemoryTool::new(save_memory.clone()))
                    .tool(SaveSharedMemoryTool::new(save_memory))
            }
            None => builder,
        };

        builder.build()
    }

    /// Builds the lead agent for an objective cycle: objective-mode instructions, `spawn_agent` bounded by the
    /// run budget, and the structured report tool as the only exit.
    #[must_use]
    pub fn build_objective_agent(&self, input: BuildObjectiveAgentInput) -> Agent {
        let run_context = &input.run_context;
        let catalog_groups =
            spawn_tool_catalog_groups(&input.prepared_connectors, has_web_search(run_context));
        let preamble = build_objective_instructions(BuildObjectiveInstructionsInput {
            additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
            spawn_tool_catalog: render_spawn_tool_catalog(&catalog_groups),
        });

        self.lead_agent_builder(LeadAgentInput {
            name: "ObjectiveRunner",
            preamble: &preamble,
            run_context,
            prepared_connectors: &input.prepared_connectors,
            catalog_groups: &catalog_groups,
            tool_concurrency: OBJECTIVE_TOOL_CONCURRENCY,
            shared_prompt_context: None,
        })
        .tool(SubmitCycleReportTool::new(input.report_slot))
        .build()
    }

    /// The lead agent and the tools both modes share: progress reporting, `spawn_agent`, and
    /// `search_web` when a web search backend is configured.
    fn lead_agent_builder(&self, input: LeadAgentInput<'_>) -> AgentBuilder<WithBuilderTools> {
        let settings = &self.config.settings;
        let builder = self
            .client
            .clone()
            .agent(settings.task_runner_model.clone())
            .name(input.name)
            .preamble(input.preamble)
            .default_max_turns(settings.task_runner_max_turns)
            .additional_params(settings.additional_params.clone());
        let builder = with_max_tokens(builder, settings.task_runner_max_tokens)
            .tool(ReportProgressTool::new(ReportProgressToolInput {
                on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
                owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
            }))
            .tool(self.spawn_agent_tool(&input));

        match &input.run_context.resources.web_search_port {
            Some(web_search_port) => builder.tool(SearchWebTool::new(Arc::clone(web_search_port))),
            None => builder,
        }
    }

    // spawn_agent replaces per-connector sub-agent tools: the lead composes each sub-agent's
    // instructions and tool selection per delegation.
    fn spawn_agent_tool(&self, input: &LeadAgentInput<'_>) -> SpawnAgentTool {
        let sub_agent_factory = SubAgentFactory::new(CreateSubAgentFactoryInput {
            client: self.sub_agent_client.clone(),
            config: self.sub_agent_config(),
        });
        let run_context = input.run_context.clone();
        let prepared_connectors = input.prepared_connectors.to_vec();
        SpawnAgentTool::new(SpawnAgentToolInput {
            agent_factory: Arc::new(move |spec: SpawnedSubAgentSpec| {
                sub_agent_factory.build_spawned_sub_agent(BuildSpawnedSubAgentInput {
                    run_context: run_context.clone(),
                    prepared_connectors: prepared_connectors.clone(),
                    spec,
                })
            }),
            available_tool_names: spawn_catalog_tool_names(input.catalog_groups),
            on_progress_event: Arc::clone(&input.run_context.execution.on_progress_event),
            tool_concurrency: input.tool_concurrency,
            shared_prompt_context: input.shared_prompt_context.clone(),
            budget: input.run_context.execution.budget.clone(),
        })
    }

    fn sub_agent_config(&self) -> SubAgentConfig {
        SubAgentConfig {
            settings: self.config.settings.clone(),
            language: self.config.instructions.language.clone(),
            additional_system_prompt: self.config.instructions.additional_system_prompt.clone(),
        }
    }
}

struct LeadAgentInput<'a> {
    name: &'static str,
    preamble: &'a str,
    run_context: &'a TaskAgentRunContext,
    prepared_connectors: &'a [Arc<dyn PreparedConnector>],
    catalog_groups: &'a [ToolCatalogGroup],
    tool_concurrency: usize,
    shared_prompt_context: Option<String>,
}

fn has_web_search(run_context: &TaskAgentRunContext) -> bool {
    run_context.resources.web_search_port.is_some()
}

pub struct BuildObjectiveAgentInput {
    pub run_context: TaskAgentRunContext,
    pub prepared_connectors: Vec<Arc<dyn PreparedConnector>>,
    pub report_slot: ReportSlot,
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
    pub budget: RunBudget,
}

impl TaskAgentExecutionContext {
    pub(crate) fn hook(&self, owner_id: String) -> AgentExecutionHook {
        AgentExecutionHook::new(
            owner_id,
            self.runtime.clone(),
            self.cancellation.clone(),
            Arc::clone(&self.logger),
            Arc::clone(&self.on_progress_event),
            self.usage_collector.clone(),
        )
        .with_budget(self.budget.clone())
    }
}

pub(super) fn with_max_tokens<ToolState>(
    builder: rig::agent::AgentBuilder<ToolState>,
    max_tokens: Option<u64>,
) -> rig::agent::AgentBuilder<ToolState> {
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
        super::prompt::build_memory_context(memory_items)
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use reili_core::cycle_agent::{ReportSlot, RunBudget, SUBMIT_CYCLE_REPORT_TOOL_NAME};
    use reili_core::knowledge::MockWebSearchPort;
    use reili_core::logger::MockLogger;
    use reili_core::messaging::slack::{
        MockSlackCanvasMemoryPort, MockSlackFileDownloadPort, MockSlackMessageSearchPort,
    };
    use reili_core::task::{
        MockTaskProgressEventPort, TaskCancellation, TaskResources, TaskRuntime,
    };
    use rig::client::CompletionClient;
    use rig::completion::message::{AssistantContent, Text};
    use rig::completion::{
        CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Prompt,
    };
    use rig::streaming::{StreamingCompletionResponse, StreamingResult};
    use serde_json::json;

    use super::{
        AgentInstructionsConfig, BuildObjectiveAgentInput, BuildTaskAgentInput,
        CreateTaskAgentFactoryInput, TaskAgentConfig, TaskAgentExecutionContext, TaskAgentFactory,
        TaskAgentRunContext,
    };
    use crate::outbound::agents::runner::provider_settings::LlmProviderSettings;
    use crate::outbound::agents::runner::usage_collector::LlmUsageCollector;

    type CapturedTools = Arc<Mutex<Vec<BTreeSet<String>>>>;

    #[derive(Clone, Default)]
    struct ToolCaptureClient {
        captured: CapturedTools,
    }

    #[derive(Clone)]
    struct ToolCaptureModel {
        captured: CapturedTools,
    }

    impl CompletionClient for ToolCaptureClient {
        type CompletionModel = ToolCaptureModel;

        fn completion_model(&self, _model: impl Into<String>) -> ToolCaptureModel {
            ToolCaptureModel {
                captured: Arc::clone(&self.captured),
            }
        }
    }

    impl CompletionModel for ToolCaptureModel {
        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse, CompletionError> {
            self.captured
                .lock()
                .expect("lock tools")
                .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
            Ok(CompletionResponse::new(
                vec![AssistantContent::Text(Text::new("done"))],
                rig::completion::Usage::new(),
                "test",
            ))
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<StreamingCompletionResponse, CompletionError> {
            let stream: StreamingResult = Box::pin(futures::stream::empty());
            Ok(StreamingCompletionResponse::stream("test", stream))
        }
    }

    fn factory(client: &ToolCaptureClient) -> TaskAgentFactory<ToolCaptureClient> {
        TaskAgentFactory::new(CreateTaskAgentFactoryInput {
            client: client.clone(),
            sub_agent_client: client.clone(),
            config: TaskAgentConfig {
                settings: LlmProviderSettings {
                    provider: "test".to_string(),
                    task_runner_model: "lead".to_string(),
                    sub_agent_model: "sub".to_string(),
                    task_runner_max_turns: 2,
                    sub_agent_max_turns: 2,
                    tool_concurrency: 2,
                    task_runner_max_tokens: None,
                    sub_agent_max_tokens: None,
                    additional_params: json!({}),
                },
                instructions: AgentInstructionsConfig {
                    language: "English".to_string(),
                    additional_system_prompt: None,
                },
            },
        })
    }

    fn run_context(with_optional_ports: bool) -> TaskAgentRunContext {
        TaskAgentRunContext {
            resources: Arc::new(TaskResources {
                slack_message_search_port: Arc::new(MockSlackMessageSearchPort::new()),
                slack_file_download_port: Arc::new(MockSlackFileDownloadPort::new()),
                web_search_port: with_optional_ports
                    .then(|| Arc::new(MockWebSearchPort::new()) as _),
                canvas_memory_port: with_optional_ports
                    .then(|| Arc::new(MockSlackCanvasMemoryPort::new()) as _),
            }),
            execution: TaskAgentExecutionContext {
                logger: Arc::new(MockLogger::new()),
                runtime: TaskRuntime {
                    started_at_iso: "2026-10-19T00:00:00Z".to_string(),
                    channel: "C1".to_string(),
                    thread_ts: "1.0".to_string(),
                    retry_count: 0,
                },
                cancellation: TaskCancellation::new(),
                on_progress_event: Arc::new(MockTaskProgressEventPort::new()),
                usage_collector: LlmUsageCollector::new(),
                budget: RunBudget::unlimited(),
            },
            slack_action_token: None,
            memory_items: Vec::new(),
        }
    }

    fn names(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(ToString::to_string).collect()
    }

    async fn first_request_tools(
        client: &ToolCaptureClient,
        agent: rig::agent::Agent,
    ) -> BTreeSet<String> {
        agent.prompt("go").await.expect("prompt");
        client.captured.lock().expect("lock tools")[0].clone()
    }

    #[tokio::test]
    async fn task_agent_registers_slack_memory_and_optional_tools() {
        for (with_optional_ports, expected) in [
            (
                true,
                names(&[
                    "report_progress",
                    "search_slack_messages",
                    "spawn_agent",
                    "search_web",
                    "save_memory",
                    "save_shared_memory",
                ]),
            ),
            (
                false,
                names(&["report_progress", "search_slack_messages", "spawn_agent"]),
            ),
        ] {
            let client = ToolCaptureClient::default();
            let agent = factory(&client).build(BuildTaskAgentInput {
                run_context: run_context(with_optional_ports),
                prepared_connectors: Vec::new(),
            });

            assert_eq!(first_request_tools(&client, agent).await, expected);
        }
    }

    #[tokio::test]
    async fn objective_agent_registers_the_report_tool_instead_of_slack_and_memory_tools() {
        for (with_optional_ports, expected) in [
            (
                true,
                names(&[
                    "report_progress",
                    "spawn_agent",
                    SUBMIT_CYCLE_REPORT_TOOL_NAME,
                    "search_web",
                ]),
            ),
            (
                false,
                names(&[
                    "report_progress",
                    "spawn_agent",
                    SUBMIT_CYCLE_REPORT_TOOL_NAME,
                ]),
            ),
        ] {
            let client = ToolCaptureClient::default();
            let agent = factory(&client).build_objective_agent(BuildObjectiveAgentInput {
                run_context: run_context(with_optional_ports),
                prepared_connectors: Vec::new(),
                report_slot: ReportSlot::new(),
            });

            assert_eq!(first_request_tools(&client, agent).await, expected);
        }
    }
}
