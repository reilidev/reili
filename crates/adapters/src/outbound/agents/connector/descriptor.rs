/// Stable identity for a connector: the source of the sub-agent name and description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorDescriptor {
    /// Sub-agent name, e.g. `"github_agent"`. Also used as the progress owner id.
    pub agent_name: String,
    /// Description shown on the `ProgressReportingSubAgentTool`.
    pub agent_description: String,
}

/// Shared context passed to a connector when building its sub-agent preamble.
///
/// Source-specific values (GitHub org scope, esa team) are held by the connector itself; only the
/// values common to every sub-agent live here.
#[derive(Debug, Clone)]
pub struct SubAgentPromptContext {
    pub language: String,
    pub additional_system_prompt: Option<String>,
}
