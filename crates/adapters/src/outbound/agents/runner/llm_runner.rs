use async_trait::async_trait;
use reili_core::cycle_agent::{CycleAgentOutcome, CycleAgentPort, RunCycleAgentInput};
use reili_core::error::AgentRunFailedError;
use reili_core::task::{RunTaskInput, TaskRunOutcome, TaskRunnerPort};
use rig::providers::{anthropic, openai};

use super::cycle_agent::{RunCycleAgentLlmInput, run_cycle_agent};
use super::environment::AgentEnvironment;
use super::provider_settings::LlmProviderSettings;
use super::providers::bedrock_mantle::BedrockMantleHttpClient;
use super::task_runner::{RunLlmTaskRunnerInput, run_task};
use crate::outbound::agents::connector::ConnectorSet;

pub(crate) struct AgentClients<C> {
    pub(crate) lead: C,
    pub(crate) sub_agent: C,
}

/// The rig clients a backend runs on. rig agents are generic over the client type, so this enum
/// is the one place that turns a configured provider into a concrete type.
pub(crate) enum LlmClients {
    OpenAi(AgentClients<openai::Client>),
    Anthropic(AgentClients<anthropic::Client>),
    Bedrock(AgentClients<rig_bedrock::client::Client>),
    BedrockMantleOpenAi(AgentClients<openai::Client<BedrockMantleHttpClient>>),
    BedrockMantleAnthropic(AgentClients<anthropic::Client<BedrockMantleHttpClient>>),
    VertexAi(AgentClients<rig_vertexai::Client>),
}

macro_rules! with_clients {
    ($clients:expr, $bound:ident => $body:expr) => {
        match $clients {
            LlmClients::OpenAi($bound) => $body,
            LlmClients::Anthropic($bound) => $body,
            LlmClients::Bedrock($bound) => $body,
            LlmClients::BedrockMantleOpenAi($bound) => $body,
            LlmClients::BedrockMantleAnthropic($bound) => $body,
            LlmClients::VertexAi($bound) => $body,
        }
    };
}

/// A configured LLM provider: its clients and the settings its agents run with.
pub struct LlmBackend {
    pub(crate) clients: LlmClients,
    pub(crate) settings: LlmProviderSettings,
}

pub struct LlmRunnerInput {
    pub backend: LlmBackend,
    pub connectors: ConnectorSet,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

/// Runs Slack tasks and objective cycles on any configured provider.
pub struct LlmRunner {
    backend: LlmBackend,
    connectors: ConnectorSet,
    language: String,
    additional_system_prompt: Option<String>,
}

impl LlmRunner {
    #[must_use]
    pub fn new(input: LlmRunnerInput) -> Self {
        Self {
            backend: input.backend,
            connectors: input.connectors,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }
    }

    fn environment<C: Clone>(&self, clients: &AgentClients<C>) -> AgentEnvironment<C> {
        AgentEnvironment {
            client: clients.lead.clone(),
            sub_agent_client: clients.sub_agent.clone(),
            settings: self.backend.settings.clone(),
            connectors: self.connectors.clone(),
            language: self.language.clone(),
            additional_system_prompt: self.additional_system_prompt.clone(),
        }
    }
}

#[async_trait]
impl TaskRunnerPort for LlmRunner {
    async fn run(&self, input: RunTaskInput) -> Result<TaskRunOutcome, AgentRunFailedError> {
        with_clients!(&self.backend.clients, clients => {
            run_task(RunLlmTaskRunnerInput {
                environment: self.environment(clients),
                run: input,
            })
            .await
        })
    }
}

#[async_trait]
impl CycleAgentPort for LlmRunner {
    async fn run(
        &self,
        input: RunCycleAgentInput,
    ) -> Result<CycleAgentOutcome, AgentRunFailedError> {
        with_clients!(&self.backend.clients, clients => {
            run_cycle_agent(RunCycleAgentLlmInput {
                environment: self.environment(clients),
                run: input,
            })
            .await
        })
    }
}
