use std::sync::Arc;

use reili_core::investigation::{
    InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
};
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
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
}

impl<M, P> ProgressReportingSubAgentTool<M, P>
where
    M: CompletionModel,
    P: rig::agent::PromptHook<M>,
{
    pub fn new(
        agent: Agent<M, P>,
        owner_id: String,
        on_progress_event: Arc<dyn InvestigationProgressEventPort>,
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
            .publish(InvestigationProgressEventInput {
                owner_id: self.owner_id.clone(),
                event: InvestigationProgressEvent::MessageOutputCreated,
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
            "
            Prompt a sub-agent to do a task for you.

            Agent name: {name}
            Agent description: {description}
            Agent system prompt: {sysprompt}
            ",
            name = self.name(),
            description = self.agent.description.clone().unwrap_or_default(),
            sysprompt = self.agent.preamble.clone().unwrap_or_default()
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

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::investigation::{
        InvestigationProgressEvent, InvestigationProgressEventInput, InvestigationProgressEventPort,
    };
    use rig::client::{CompletionClient, ProviderClient};

    use super::ProgressReportingSubAgentTool;

    struct MockProgressEventPort {
        calls: Arc<Mutex<Vec<InvestigationProgressEventInput>>>,
    }

    #[async_trait]
    impl InvestigationProgressEventPort for MockProgressEventPort {
        async fn publish(&self, input: InvestigationProgressEventInput) -> Result<(), PortError> {
            self.calls.lock().expect("lock calls").push(input);
            Ok(())
        }
    }

    #[tokio::test]
    async fn publishes_message_output_created_event() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let client = rig::providers::openai::Client::from_val("test-key".into());
        let agent = client
            .agent("gpt-5.3-codex")
            .name("investigate_datadog")
            .build();
        let tool = ProgressReportingSubAgentTool::new(
            agent,
            "investigate_datadog".to_string(),
            Arc::new(MockProgressEventPort {
                calls: Arc::clone(&calls),
            }),
        );

        tool.publish_message_output_created().await;

        assert_eq!(
            calls.lock().expect("lock calls").as_slice(),
            &[InvestigationProgressEventInput {
                owner_id: "investigate_datadog".to_string(),
                event: InvestigationProgressEvent::MessageOutputCreated,
            }]
        );
    }
}
