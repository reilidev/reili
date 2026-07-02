use std::collections::BTreeSet;
use std::sync::Arc;

use reili_core::task::{TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort};
use rig::agent::Agent;
use rig::agent::PromptHook;
use rig::completion::{CompletionModel, Prompt, PromptError, ToolDefinition};
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

/// Longest accepted sub-agent name; keeps progress owner ids readable in Slack updates.
const MAX_AGENT_NAME_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgentToolArgs {
    pub name: String,
    pub instructions: String,
    pub tools: Vec<String>,
    pub prompt: String,
}

/// Validated spec handed to the agent factory: everything the assembly side needs to compose the
/// preamble and resolve the selected tools.
pub struct SpawnedSubAgentSpec {
    pub owner_id: String,
    pub name: String,
    pub instructions: String,
    pub tool_names: Vec<String>,
}

#[derive(Debug)]
pub enum SpawnAgentToolError {
    InvalidSpec(String),
    Prompt(PromptError),
}

impl std::fmt::Display for SpawnAgentToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSpec(message) => write!(f, "{message}"),
            Self::Prompt(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SpawnAgentToolError {}

impl From<PromptError> for SpawnAgentToolError {
    fn from(error: PromptError) -> Self {
        Self::Prompt(error)
    }
}

pub struct SpawnAgentToolInput<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    pub agent_factory: Arc<dyn Fn(SpawnedSubAgentSpec) -> Agent<M, P> + Send + Sync>,
    /// Tool names the lead may select, in catalog order. Used for validation and error messages.
    pub available_tool_names: Vec<String>,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub tool_concurrency: usize,
    pub shared_prompt_context: Option<String>,
}

#[derive(Clone)]
pub struct SpawnAgentTool<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    agent_factory: Arc<dyn Fn(SpawnedSubAgentSpec) -> Agent<M, P> + Send + Sync>,
    available_tool_names: Vec<String>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    tool_concurrency: usize,
    shared_prompt_context: Option<String>,
}

impl<M, P> SpawnAgentTool<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    pub fn new(input: SpawnAgentToolInput<M, P>) -> Self {
        Self {
            agent_factory: input.agent_factory,
            available_tool_names: input.available_tool_names,
            on_progress_event: input.on_progress_event,
            tool_concurrency: input.tool_concurrency.max(1),
            shared_prompt_context: input
                .shared_prompt_context
                .map(|context| context.trim().to_string())
                .filter(|context| !context.is_empty()),
        }
    }

    fn validate_args(
        &self,
        args: &SpawnAgentToolArgs,
    ) -> Result<SpawnedSubAgentSpec, SpawnAgentToolError> {
        let name = sanitize_agent_name(&args.name).ok_or_else(|| {
            SpawnAgentToolError::InvalidSpec(
                "name must contain at least one alphanumeric character; use a short snake_case name describing the sub-agent's scope".to_string(),
            )
        })?;

        let instructions = args.instructions.trim();
        if instructions.is_empty() {
            return Err(SpawnAgentToolError::InvalidSpec(
                "instructions must describe the sub-agent's mission: role, goal, relevant background, and what a good answer looks like".to_string(),
            ));
        }

        if args.tools.is_empty() {
            return Err(SpawnAgentToolError::InvalidSpec(format!(
                "tools must select at least one tool from the catalog. Available tools: {}",
                self.available_tool_names.join(", "),
            )));
        }

        let available: BTreeSet<&str> = self
            .available_tool_names
            .iter()
            .map(String::as_str)
            .collect();
        let mut unknown_names: Vec<&str> = args
            .tools
            .iter()
            .map(String::as_str)
            .filter(|name| !available.contains(name))
            .collect();
        unknown_names.sort_unstable();
        unknown_names.dedup();
        if !unknown_names.is_empty() {
            return Err(SpawnAgentToolError::InvalidSpec(format!(
                "unknown tools: {}. Available tools: {}",
                unknown_names.join(", "),
                self.available_tool_names.join(", "),
            )));
        }

        let mut tool_names = args.tools.clone();
        tool_names.dedup();

        Ok(SpawnedSubAgentSpec {
            owner_id: format!("{name}:{}", Uuid::new_v4().simple()),
            name,
            instructions: instructions.to_string(),
            tool_names,
        })
    }

    fn build_prompt(&self, prompt: String) -> String {
        match self.shared_prompt_context.as_deref() {
            Some(context) => format!("{context}\n\n# Delegated Task\n{prompt}"),
            None => prompt,
        }
    }

    async fn publish_message_output_created(&self, owner_id: &str) {
        let publish_result = self
            .on_progress_event
            .publish(TaskProgressEventInput {
                owner_id: owner_id.to_string(),
                event: TaskProgressEvent::MessageOutputCreated,
            })
            .await;
        if let Err(error) = publish_result {
            tracing::error!(
                owner_id,
                error = error.message,
                "Failed to publish spawned sub agent message output progress event",
            );
        }
    }
}

impl<M, P> Tool for SpawnAgentTool<M, P>
where
    M: CompletionModel + 'static,
    P: PromptHook<M> + 'static,
{
    const NAME: &'static str = "spawn_agent";

    type Error = SpawnAgentToolError;
    type Args = SpawnAgentToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Spawn a one-shot sub-agent that runs with only the tools you select from the sub-agent tool catalog and returns its final report. Compose the sub-agent for the delegated mission: task-specific instructions plus the minimal tool set it needs. Run multiple spawn_agent calls in parallel for independent scopes.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short snake_case name describing the sub-agent's scope, e.g. checkout_error_logs. Shown in progress updates."
                    },
                    "instructions": {
                        "type": "string",
                        "description": "The sub-agent's mission: its role, the goal, relevant background from the current task, hypotheses to test, and what a good answer looks like. Language, progress reporting, memory handling, and mandatory scope rules are added automatically — do not repeat them."
                    },
                    "tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tool names from the sub-agent tool catalog. Select the minimal set the mission needs; mixing tools from different sources is allowed."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The concrete delegated task with its inputs: service names, time ranges, links, and error snippets."
                    }
                },
                "required": ["name", "instructions", "tools", "prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let spec = self.validate_args(&args)?;
        let owner_id = spec.owner_id.clone();
        let agent = (self.agent_factory)(spec);
        let prompt = self.build_prompt(args.prompt);
        let output = agent
            .prompt(prompt)
            .with_tool_concurrency(self.tool_concurrency)
            .await?;
        self.publish_message_output_created(&owner_id).await;
        Ok(output)
    }
}

/// Normalizes a lead-provided sub-agent name into a progress owner id prefix: keeps ASCII
/// alphanumerics, `_`, and `-`; maps everything else to `_`. Returns `None` when nothing usable
/// remains.
fn sanitize_agent_name(name: &str) -> Option<String> {
    let sanitized: String = name
        .trim()
        .chars()
        .take(MAX_AGENT_NAME_LENGTH)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();

    if sanitized
        .chars()
        .any(|character| character.is_ascii_alphanumeric())
    {
        Some(sanitized)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::task::{MockTaskProgressEventPort, TaskProgressEvent, TaskProgressEventInput};
    use rig::OneOrMany;
    use rig::agent::AgentBuilder;
    use rig::completion::{
        CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
    };
    use rig::message::{AssistantContent, Message, Text, UserContent};
    use rig::streaming::{StreamingCompletionResponse, StreamingResult};
    use rig::tool::Tool;

    use super::{
        SpawnAgentTool, SpawnAgentToolArgs, SpawnAgentToolError, SpawnAgentToolInput,
        sanitize_agent_name,
    };

    #[derive(Clone)]
    struct PromptCaptureModel {
        captured_prompts: Arc<Mutex<Vec<String>>>,
    }

    #[allow(refining_impl_trait)]
    impl CompletionModel for PromptCaptureModel {
        type Response = ();
        type StreamingResponse = ();
        type Client = ();

        fn make(_: &Self::Client, _: impl Into<String>) -> Self {
            Self {
                captured_prompts: Arc::new(Mutex::new(Vec::new())),
            }
        }

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let prompt = request
                .chat_history
                .iter()
                .last()
                .and_then(message_text)
                .unwrap_or_default()
                .to_string();
            self.captured_prompts
                .lock()
                .expect("lock prompts")
                .push(prompt);

            Ok(CompletionResponse {
                choice: OneOrMany::one(AssistantContent::Text(Text {
                    text: "done".to_string(),
                })),
                usage: Usage::new(),
                raw_response: (),
                message_id: Some("text-message".to_string()),
            })
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
            let stream: StreamingResult<()> = Box::pin(futures::stream::empty());
            Ok(StreamingCompletionResponse::stream(stream))
        }
    }

    fn message_text(message: &Message) -> Option<&str> {
        match message {
            Message::User { content } => content.iter().find_map(|item| match item {
                UserContent::Text(Text { text }) => Some(text.as_str()),
                _ => None,
            }),
            Message::System { content } => Some(content.as_str()),
            Message::Assistant { .. } => None,
        }
    }

    struct RecordedSpawn {
        owner_id: String,
        instructions: String,
        tool_names: Vec<String>,
    }

    struct SpawnToolHarness {
        tool: SpawnAgentTool<PromptCaptureModel, ()>,
        spawned_specs: Arc<Mutex<Vec<RecordedSpawn>>>,
        captured_prompts: Arc<Mutex<Vec<String>>>,
        progress_events: Arc<Mutex<Vec<TaskProgressEventInput>>>,
    }

    fn spawn_tool_harness(
        available_tool_names: Vec<&str>,
        expected_publish_count: usize,
        shared_prompt_context: Option<String>,
    ) -> SpawnToolHarness {
        let progress_events = Arc::new(Mutex::new(Vec::new()));
        let publish_events = Arc::clone(&progress_events);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(expected_publish_count)
            .returning(move |input| {
                publish_events.lock().expect("lock events").push(input);
                Ok(())
            });

        let spawned_specs = Arc::new(Mutex::new(Vec::new()));
        let captured_prompts = Arc::new(Mutex::new(Vec::new()));
        let agent_factory = {
            let spawned_specs = Arc::clone(&spawned_specs);
            let captured_prompts = Arc::clone(&captured_prompts);
            Arc::new(move |spec: super::SpawnedSubAgentSpec| {
                spawned_specs
                    .lock()
                    .expect("lock specs")
                    .push(RecordedSpawn {
                        owner_id: spec.owner_id.clone(),
                        instructions: spec.instructions.clone(),
                        tool_names: spec.tool_names.clone(),
                    });
                AgentBuilder::new(PromptCaptureModel {
                    captured_prompts: Arc::clone(&captured_prompts),
                })
                .name(&spec.name)
                .build()
            })
        };

        let tool = SpawnAgentTool::new(SpawnAgentToolInput {
            agent_factory,
            available_tool_names: available_tool_names
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            on_progress_event: Arc::new(progress_event_port),
            tool_concurrency: 4,
            shared_prompt_context,
        });

        SpawnToolHarness {
            tool,
            spawned_specs,
            captured_prompts,
            progress_events,
        }
    }

    fn valid_args() -> SpawnAgentToolArgs {
        SpawnAgentToolArgs {
            name: "checkout_error_logs".to_string(),
            instructions: "Investigate the checkout error spike.".to_string(),
            tools: vec!["search_datadog_logs".to_string(), "search_code".to_string()],
            prompt: "Find the error pattern for checkout-api since 09:00 UTC.".to_string(),
        }
    }

    #[tokio::test]
    async fn spawns_sub_agent_with_selected_tools_and_publishes_progress() {
        let harness = spawn_tool_harness(
            vec!["search_datadog_logs", "search_code", "search_web"],
            1,
            None,
        );

        let output = harness
            .tool
            .call(valid_args())
            .await
            .expect("spawn should succeed");

        assert_eq!(output, "done");
        let specs = harness.spawned_specs.lock().expect("lock specs");
        assert_eq!(specs.len(), 1);
        let spawned = &specs[0];
        assert!(spawned.owner_id.starts_with("checkout_error_logs:"));
        let uuid_part = spawned
            .owner_id
            .strip_prefix("checkout_error_logs:")
            .expect("owner id should contain prefix");
        uuid::Uuid::parse_str(uuid_part).expect("owner id should contain UUID");
        assert_eq!(
            spawned.instructions,
            "Investigate the checkout error spike."
        );
        assert_eq!(
            spawned.tool_names,
            vec!["search_datadog_logs".to_string(), "search_code".to_string()]
        );
        assert_eq!(
            harness
                .progress_events
                .lock()
                .expect("lock events")
                .as_slice(),
            &[TaskProgressEventInput {
                owner_id: spawned.owner_id.clone(),
                event: TaskProgressEvent::MessageOutputCreated,
            }]
        );
    }

    #[tokio::test]
    async fn rejects_unknown_tool_names_and_lists_available_tools() {
        let harness = spawn_tool_harness(vec!["search_datadog_logs", "search_code"], 0, None);
        let mut args = valid_args();
        args.tools = vec![
            "search_datadog_logs".to_string(),
            "search_datadog_lgos".to_string(),
        ];

        let error = harness
            .tool
            .call(args)
            .await
            .expect_err("unknown tool should fail");

        let SpawnAgentToolError::InvalidSpec(message) = error else {
            panic!("expected invalid spec error");
        };
        assert!(message.contains("unknown tools: search_datadog_lgos"));
        assert!(message.contains("Available tools: search_datadog_logs, search_code"));
        assert!(harness.spawned_specs.lock().expect("lock specs").is_empty());
    }

    #[tokio::test]
    async fn rejects_empty_tool_selection() {
        let harness = spawn_tool_harness(vec!["search_code"], 0, None);
        let mut args = valid_args();
        args.tools = Vec::new();

        let error = harness
            .tool
            .call(args)
            .await
            .expect_err("empty tools should fail");

        let SpawnAgentToolError::InvalidSpec(message) = error else {
            panic!("expected invalid spec error");
        };
        assert!(message.contains("at least one tool"));
    }

    #[tokio::test]
    async fn rejects_blank_instructions() {
        let harness = spawn_tool_harness(vec!["search_code"], 0, None);
        let mut args = valid_args();
        args.tools = vec!["search_code".to_string()];
        args.instructions = "   ".to_string();

        let error = harness
            .tool
            .call(args)
            .await
            .expect_err("blank instructions should fail");

        assert!(matches!(error, SpawnAgentToolError::InvalidSpec(_)));
    }

    #[tokio::test]
    async fn prepends_shared_prompt_context_to_spawned_prompt() {
        let harness = spawn_tool_harness(
            vec!["search_datadog_logs", "search_code"],
            1,
            Some("# Memory Context\nmemory:\n- service: checkout-api".to_string()),
        );

        let output = harness
            .tool
            .call(valid_args())
            .await
            .expect("spawn should succeed");

        assert_eq!(output, "done");
        let prompts = harness.captured_prompts.lock().expect("lock prompts");
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0],
            "# Memory Context\nmemory:\n- service: checkout-api\n\n# Delegated Task\nFind the error pattern for checkout-api since 09:00 UTC."
        );
    }

    #[test]
    fn sanitizes_agent_names_for_owner_ids() {
        assert_eq!(
            sanitize_agent_name(" checkout error logs "),
            Some("checkout_error_logs".to_string())
        );
        assert_eq!(
            sanitize_agent_name("checkout_error_logs"),
            Some("checkout_error_logs".to_string())
        );
        assert_eq!(sanitize_agent_name("!!!"), None);
        assert_eq!(sanitize_agent_name("   "), None);
    }
}
