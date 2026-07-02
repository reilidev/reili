use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use super::catalog::ToolCatalogGroup;
use super::error::ConnectorPrepareError;
use super::prompt_fact::ConnectorPromptFact;

/// One source = one factory. Owns connection (async, fallible).
#[async_trait]
pub trait ConnectorFactory: Send + Sync {
    /// Establish the transport and fetch tools. Failure semantics are expressed in the error type.
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError>;
}

/// A connected handle. Cheap and synchronous to query during the assembly phase.
pub trait PreparedConnector: Send + Sync {
    /// Tools exposed to a dynamically spawned sub-agent.
    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>>;

    /// Tools exposed directly to the lead (only Datadog is non-empty; default is empty).
    fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        Vec::new()
    }

    /// Catalog of tools this connector can supply to a dynamically spawned sub-agent.
    /// Entries must match the names of tools returned by [`Self::sub_agent_tools`].
    fn spawn_tool_catalog(&self) -> ToolCatalogGroup;

    /// Guardrail instructions composed into a spawned sub-agent's preamble when at least one
    /// of this connector's tools is selected (mandatory scope rules, source usage notes).
    fn spawn_guardrails(&self) -> Option<String> {
        None
    }

    /// Source-specific facts contributed to the lead prompt's "Current context" block.
    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        Vec::new()
    }
}
