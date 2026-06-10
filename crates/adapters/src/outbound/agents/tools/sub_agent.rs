use std::sync::Arc;

use reili_core::task::{TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort};
use rig::agent::Agent;
use rig::agent::PromptHook;
use rig::completion::{CompletionModel, Prompt, PromptError, ToolDefinition};
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressReportingSubAgentToolArgs {
    prompt: String,
}

pub struct ProgressReportingSubAgentToolInput<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    pub agent_factory: Arc<dyn Fn(String) -> Agent<M, P> + Send + Sync>,
    pub agent_name: String,
    pub agent_description: Option<String>,
    pub owner_id: String,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub tool_concurrency: usize,
    pub shared_prompt_context: Option<String>,
}

#[derive(Clone)]
pub struct ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    agent_factory: Arc<dyn Fn(String) -> Agent<M, P> + Send + Sync>,
    agent_name: String,
    agent_description: Option<String>,
    owner_id: String,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    tool_concurrency: usize,
    shared_prompt_context: Option<String>,
}

impl<M, P> ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel,
    P: PromptHook<M>,
{
    pub fn new(input: ProgressReportingSubAgentToolInput<M, P>) -> Self {
        Self {
            agent_factory: input.agent_factory,
            agent_name: input.agent_name,
            agent_description: input.agent_description,
            owner_id: input.owner_id,
            on_progress_event: input.on_progress_event,
            tool_concurrency: input.tool_concurrency.max(1),
            shared_prompt_context: input
                .shared_prompt_context
                .map(|context| context.trim().to_string())
                .filter(|context| !context.is_empty()),
        }
    }

    fn build_prompt(&self, prompt: String) -> String {
        match self.shared_prompt_context.as_deref() {
            Some(context) => format!("{context}\n\n# Delegated Task\n{prompt}"),
            None => prompt,
        }
    }

    fn build_invocation_owner_id(&self) -> String {
        format!("{}:{}", self.owner_id, Uuid::new_v4().simple())
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
                "Failed to publish sub agent message output progress event",
            );
        }
    }
}

impl<M, P> Tool for ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel + 'static,
    P: PromptHook<M> + 'static,
{
    const NAME: &'static str = "progress_reporting_sub_agent_tool";

    type Error = PromptError;
    type Args = ProgressReportingSubAgentToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let description = format!(
            "Prompt the {name} sub-agent to do a task for you.\n\nAgent description: {agent_description}",
            name = self.name(),
            agent_description = self.agent_description.as_deref().unwrap_or_default(),
        );
        ToolDefinition {
            name: self.name(),
            description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The prompt for the sub-agent to call."
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let invocation_owner_id = self.build_invocation_owner_id();
        let agent = (self.agent_factory)(invocation_owner_id.clone());
        let prompt = self.build_prompt(args.prompt);
        let output = agent
            .prompt(prompt)
            .with_tool_concurrency(self.tool_concurrency)
            .await?;
        self.publish_message_output_created(&invocation_owner_id)
            .await;
        Ok(output)
    }

    fn name(&self) -> String {
        self.agent_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use reili_core::task::{MockTaskProgressEventPort, TaskProgressEvent, TaskProgressEventInput};
    use rig::OneOrMany;
    use rig::agent::AgentBuilder;
    use rig::completion::{
        CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
    };
    use rig::message::{AssistantContent, Message, Text, ToolCall, ToolFunction, UserContent};
    use rig::streaming::{StreamingCompletionResponse, StreamingResult};
    use rig::tool::Tool;
    use serde::Deserialize;
    use serde_json::json;

    use super::{
        ProgressReportingSubAgentTool, ProgressReportingSubAgentToolArgs,
        ProgressReportingSubAgentToolInput,
    };

    #[tokio::test]
    async fn uses_unique_invocation_owner_id_for_each_sub_agent_call() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(2)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let built_owner_ids = Arc::new(Mutex::new(Vec::new()));
        let captured_prompts = Arc::new(Mutex::new(Vec::new()));
        let agent_factory = {
            let built_owner_ids = Arc::clone(&built_owner_ids);
            let captured_prompts = Arc::clone(&captured_prompts);
            Arc::new(move |owner_id: String| {
                built_owner_ids
                    .lock()
                    .expect("lock built owner ids")
                    .push(owner_id);
                AgentBuilder::new(PromptCaptureModel {
                    captured_prompts: Arc::clone(&captured_prompts),
                })
                .name("investigate_datadog")
                .build()
            })
        };
        let tool = ProgressReportingSubAgentTool::new(ProgressReportingSubAgentToolInput {
            agent_factory,
            agent_name: "investigate_datadog".to_string(),
            agent_description: Some("Datadog sub-agent".to_string()),
            owner_id: "investigate_datadog".to_string(),
            on_progress_event: Arc::new(progress_event_port),
            tool_concurrency: 8,
            shared_prompt_context: None,
        });

        let first_output = tool
            .call(ProgressReportingSubAgentToolArgs {
                prompt: "first".to_string(),
            })
            .await
            .expect("first sub-agent prompt should succeed");
        let second_output = tool
            .call(ProgressReportingSubAgentToolArgs {
                prompt: "second".to_string(),
            })
            .await
            .expect("second sub-agent prompt should succeed");

        assert_eq!(first_output, "done");
        assert_eq!(second_output, "done");
        let built_owner_ids = built_owner_ids.lock().expect("lock built owner ids");
        assert_eq!(built_owner_ids.len(), 2);
        assert_ne!(built_owner_ids[0], built_owner_ids[1]);
        for owner_id in built_owner_ids.iter() {
            assert!(owner_id.starts_with("investigate_datadog:"));
            let uuid_part = owner_id
                .strip_prefix("investigate_datadog:")
                .expect("owner id should contain prefix");
            uuid::Uuid::parse_str(uuid_part).expect("owner id should contain UUID");
        }
        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[
                TaskProgressEventInput {
                    owner_id: built_owner_ids[0].clone(),
                    event: TaskProgressEvent::MessageOutputCreated,
                },
                TaskProgressEventInput {
                    owner_id: built_owner_ids[1].clone(),
                    event: TaskProgressEvent::MessageOutputCreated,
                },
            ]
        );
    }

    #[tokio::test]
    async fn runs_sub_agent_tool_calls_with_configured_concurrency() {
        let active_calls = Arc::new(AtomicUsize::new(0));
        let max_active_calls = Arc::new(AtomicUsize::new(0));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let agent = AgentBuilder::new(ParallelToolCallModel::new())
            .name("investigate_datadog")
            .tool(ConcurrentProbeTool {
                active_calls: Arc::clone(&active_calls),
                max_active_calls: Arc::clone(&max_active_calls),
            })
            .build();
        let agent_factory = Arc::new(move |_owner_id| agent.clone());
        let tool = ProgressReportingSubAgentTool::new(ProgressReportingSubAgentToolInput {
            agent_factory,
            agent_name: "investigate_datadog".to_string(),
            agent_description: Some("Datadog sub-agent".to_string()),
            owner_id: "investigate_datadog".to_string(),
            on_progress_event: Arc::new(progress_event_port),
            tool_concurrency: 2,
            shared_prompt_context: None,
        });

        let output = tool
            .call(ProgressReportingSubAgentToolArgs {
                prompt: "run probes".to_string(),
            })
            .await
            .expect("sub-agent prompt should succeed");

        assert_eq!(output, "done");
        assert_eq!(max_active_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn prepends_shared_prompt_context_to_sub_agent_prompt() {
        let captured_prompts = Arc::new(Mutex::new(Vec::new()));
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(|_| Ok(()));
        let agent = AgentBuilder::new(PromptCaptureModel {
            captured_prompts: Arc::clone(&captured_prompts),
        })
        .name("investigate_datadog")
        .build();
        let agent_factory = Arc::new(move |_owner_id| agent.clone());
        let tool = ProgressReportingSubAgentTool::new(ProgressReportingSubAgentToolInput {
            agent_factory,
            agent_name: "investigate_datadog".to_string(),
            agent_description: Some("Datadog sub-agent".to_string()),
            owner_id: "investigate_datadog".to_string(),
            on_progress_event: Arc::new(progress_event_port),
            tool_concurrency: 1,
            shared_prompt_context: Some(
                "# Memory Context\nsource: https://slack/memory\nmemory:\n- service: checkout-api"
                    .to_string(),
            ),
        });

        let output = tool
            .call(ProgressReportingSubAgentToolArgs {
                prompt: "Investigate the checkout-api alert.".to_string(),
            })
            .await
            .expect("sub-agent prompt should succeed");

        assert_eq!(output, "done");
        let prompts = captured_prompts.lock().expect("lock prompts");
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0],
            "# Memory Context\nsource: https://slack/memory\nmemory:\n- service: checkout-api\n\n# Delegated Task\nInvestigate the checkout-api alert."
        );
    }

    #[derive(Clone)]
    struct ParallelToolCallModel {
        turn: Arc<AtomicUsize>,
    }

    impl ParallelToolCallModel {
        fn new() -> Self {
            Self {
                turn: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

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

    #[allow(refining_impl_trait)]
    impl CompletionModel for ParallelToolCallModel {
        type Response = ();
        type StreamingResponse = ();
        type Client = ();

        fn make(_: &Self::Client, _: impl Into<String>) -> Self {
            Self::new()
        }

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let turn = self.turn.fetch_add(1, Ordering::SeqCst);
            if turn == 0 {
                return Ok(CompletionResponse {
                    choice: OneOrMany::many(vec![
                        AssistantContent::ToolCall(ToolCall::new(
                            "tool-call-1".to_string(),
                            ToolFunction::new("concurrent_probe".to_string(), json!({"id": "1"})),
                        )),
                        AssistantContent::ToolCall(ToolCall::new(
                            "tool-call-2".to_string(),
                            ToolFunction::new("concurrent_probe".to_string(), json!({"id": "2"})),
                        )),
                    ])
                    .expect("non-empty tool calls"),
                    usage: Usage::new(),
                    raw_response: (),
                    message_id: Some("tool-message".to_string()),
                });
            }

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

    #[derive(Clone)]
    struct ConcurrentProbeTool {
        active_calls: Arc<AtomicUsize>,
        max_active_calls: Arc<AtomicUsize>,
    }

    #[derive(Debug, Deserialize)]
    struct ConcurrentProbeArgs {
        id: String,
    }

    #[derive(Debug)]
    struct ConcurrentProbeError;

    impl std::fmt::Display for ConcurrentProbeError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "probe failed")
        }
    }

    impl std::error::Error for ConcurrentProbeError {}

    impl Tool for ConcurrentProbeTool {
        const NAME: &'static str = "concurrent_probe";

        type Error = ConcurrentProbeError;
        type Args = ConcurrentProbeArgs;
        type Output = String;

        async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
            rig::completion::ToolDefinition {
                name: Self::NAME.to_string(),
                description: "Records concurrent tool execution in tests.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    },
                    "required": ["id"]
                }),
            }
        }

        async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
            let active = self.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let _ =
                self.max_active_calls
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                        Some(current.max(active))
                    });
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.active_calls.fetch_sub(1, Ordering::SeqCst);

            Ok(args.id)
        }
    }
}
