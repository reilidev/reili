use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use super::descriptor::{ConnectorDescriptor, SpecialistPromptContext};
use super::error::ConnectorPrepareError;
use super::prompt_fact::ConnectorPromptFact;

/// One source = one factory. Owns connection (async, fallible).
#[async_trait]
pub trait ConnectorFactory: Send + Sync {
    fn descriptor(&self) -> &ConnectorDescriptor;

    /// Establish the transport and fetch tools. Failure semantics are expressed in the error type.
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError>;
}

/// A connected handle. Cheap and synchronous to query during the assembly phase.
pub trait PreparedConnector: Send + Sync {
    fn descriptor(&self) -> &ConnectorDescriptor;

    /// Tools exposed to the specialist sub-agent.
    fn specialist_tools(&self) -> Vec<Box<dyn ToolDyn>>;

    /// Tools exposed directly to the lead (only Datadog is non-empty; default is empty).
    fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        Vec::new()
    }

    /// Specialist preamble for this connector.
    fn specialist_preamble(&self, context: &SpecialistPromptContext) -> String;

    /// Source-specific facts contributed to the lead prompt's "Current context" block.
    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        Vec::new()
    }
}
