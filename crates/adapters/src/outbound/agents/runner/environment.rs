use std::sync::Arc;

use reili_core::error::AgentRunFailedError;
use rig::prelude::CompletionClient;

use super::provider_settings::LlmProviderSettings;
use super::usage_collector::LlmUsageCollector;
use crate::outbound::agents::connector::{
    ConnectorPrepareError, ConnectorPromptFact, ConnectorSet, PreparedConnector,
};
use crate::outbound::agents::task_agent::{
    AgentInstructionsConfig, CreateTaskAgentFactoryInput, TaskAgentConfig, TaskAgentFactory,
};

/// What every agent run needs besides its request: the clients, provider settings, connectors,
/// and the configured language and instructions.
pub struct AgentEnvironment<C> {
    pub client: C,
    pub sub_agent_client: C,
    pub settings: LlmProviderSettings,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

impl<C: CompletionClient + Clone> AgentEnvironment<C> {
    pub(crate) fn agent_factory(&self) -> TaskAgentFactory<C> {
        TaskAgentFactory::new(CreateTaskAgentFactoryInput {
            client: self.client.clone(),
            sub_agent_client: self.sub_agent_client.clone(),
            config: TaskAgentConfig {
                settings: self.settings.clone(),
                instructions: AgentInstructionsConfig {
                    language: self.language.clone(),
                    additional_system_prompt: self.additional_system_prompt.clone(),
                },
            },
        })
    }
}

impl<C> AgentEnvironment<C> {
    /// A connector that cannot connect fails the run permanently; any other failure can be retried.
    pub(crate) async fn prepare_connectors(
        &self,
        usage_collector: &LlmUsageCollector,
    ) -> Result<Vec<Arc<dyn PreparedConnector>>, AgentRunFailedError> {
        self.connectors.prepare_all().await.map_err(|error| {
            let usage = usage_collector.snapshot();
            match error {
                ConnectorPrepareError::ConnectionFailed { message } => {
                    AgentRunFailedError::new_permanent(usage, message)
                }
                ConnectorPrepareError::Other(port_error) => {
                    AgentRunFailedError::new(usage, port_error.message)
                }
            }
        })
    }
}

pub(crate) fn prompt_facts(
    prepared_connectors: &[Arc<dyn PreparedConnector>],
) -> Vec<ConnectorPromptFact> {
    prepared_connectors
        .iter()
        .flat_map(|connector| connector.prompt_facts())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use serde_json::json;

    use super::AgentEnvironment;
    use crate::outbound::agents::connector::{
        ConnectorFactory, ConnectorPrepareError, ConnectorSet, PreparedConnector,
    };
    use crate::outbound::agents::runner::provider_settings::LlmProviderSettings;
    use crate::outbound::agents::runner::usage_collector::LlmUsageCollector;

    struct FailingConnector(fn() -> ConnectorPrepareError);

    #[async_trait]
    impl ConnectorFactory for FailingConnector {
        async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
            Err((self.0)())
        }
    }

    fn environment(failure: fn() -> ConnectorPrepareError) -> AgentEnvironment<()> {
        AgentEnvironment {
            client: (),
            sub_agent_client: (),
            settings: LlmProviderSettings {
                provider: "test".to_string(),
                task_runner_model: "lead".to_string(),
                sub_agent_model: "sub".to_string(),
                task_runner_max_turns: 1,
                sub_agent_max_turns: 1,
                tool_concurrency: 1,
                task_runner_max_tokens: None,
                sub_agent_max_tokens: None,
                additional_params: json!({}),
            },
            connectors: ConnectorSet::new(vec![Arc::new(FailingConnector(failure))]),
            language: "English".to_string(),
            additional_system_prompt: None,
        }
    }

    #[tokio::test]
    async fn connection_failures_are_permanent_and_other_failures_retryable() {
        let unreachable = environment(|| ConnectorPrepareError::ConnectionFailed {
            message: "unreachable".to_string(),
        })
        .prepare_connectors(&LlmUsageCollector::new())
        .await
        .err()
        .expect("connection failure");
        let broken = environment(|| ConnectorPrepareError::Other(PortError::new("broken")))
            .prepare_connectors(&LlmUsageCollector::new())
            .await
            .err()
            .expect("other failure");

        assert!(unreachable.is_permanent);
        assert_eq!(unreachable.cause_message, "unreachable");
        assert!(!broken.is_permanent);
        assert_eq!(broken.cause_message, "broken");
    }
}
