use std::sync::Arc;

use reili_core::task::{TaskProgressEvent, TaskProgressEventInput, TaskProgressEventPort};
use rig::agent::Agent;
use rig::completion::{CompletionModel, Prompt, PromptError, ToolDefinition};
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressReportingSubAgentToolArgs {
    prompt: String,
}

#[derive(Clone)]
pub struct ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel,
    P: rig::agent::PromptHook<M>,
{
    agent: Agent<M, P>,
    owner_id: String,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
}

impl<M, P> ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel,
    P: rig::agent::PromptHook<M>,
{
    pub fn new(
        agent: Agent<M, P>,
        owner_id: String,
        on_progress_event: Arc<dyn TaskProgressEventPort>,
    ) -> Self {
        Self {
            agent,
            owner_id,
            on_progress_event,
        }
    }

    async fn publish_message_output_created(&self) {
        let publish_result = self
            .on_progress_event
            .publish(TaskProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: TaskProgressEvent::MessageOutputCreated,
            })
            .await;
        if let Err(error) = publish_result {
            tracing::error!(
                owner_id = self.owner_id,
                error = error.message,
                "Failed to publish sub agent message output progress event",
            );
        }
    }
}

impl<M, P> Tool for ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    const NAME: &'static str = "progress_reporting_sub_agent_tool";

    type Error = PromptError;
    type Args = ProgressReportingSubAgentToolArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let description = format!(
            "Prompt the {name} sub-agent to do a task for you.\n\nAgent description: {agent_description}",
            name = self.name(),
            agent_description = self.agent.description.as_deref().unwrap_or_default(),
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
        let output = self.agent.prompt(args.prompt).await?;
        self.publish_message_output_created().await;
        Ok(output)
    }

    fn name(&self) -> String {
        self.agent
            .name
            .clone()
            .unwrap_or_else(|| Self::NAME.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use reili_core::task::{MockTaskProgressEventPort, TaskProgressEvent, TaskProgressEventInput};
    use rig::client::{CompletionClient, ProviderClient};

    use super::ProgressReportingSubAgentTool;

    #[tokio::test]
    async fn publishes_message_output_created_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let publish_calls = Arc::clone(&calls);
        let mut progress_event_port = MockTaskProgressEventPort::new();
        progress_event_port
            .expect_publish()
            .times(1)
            .returning(move |input| {
                publish_calls.lock().expect("lock calls").push(input);
                Ok(())
            });
        let client = rig::providers::openai::Client::from_val("test-key".into());
        let agent = client
            .agent("gpt-5.3-codex")
            .name("investigate_datadog")
            .build();
        let tool = ProgressReportingSubAgentTool::new(
            agent,
            "investigate_datadog".to_string(),
            Arc::new(progress_event_port),
        );

        tool.publish_message_output_created().await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[TaskProgressEventInput {
                owner_id: "investigate_datadog".to_string(),
                event: TaskProgressEvent::MessageOutputCreated,
            }]
        );
    }
}
