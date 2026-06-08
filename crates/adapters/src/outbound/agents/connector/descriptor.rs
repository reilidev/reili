/// Stable identity for a connector: the source of the sub-agent name and description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorDescriptor {
    /// Sub-agent name, e.g. `"investigate_github"`. Also used as the progress owner id.
    pub agent_name: String,
    /// Description shown on the `ProgressReportingSubAgentTool`.
    pub agent_description: String,
}

/// Shared context passed to a connector when building its specialist preamble.
///
/// Source-specific values (GitHub org scope, esa team) are held by the connector itself; only the
/// values common to every specialist live here.
#[derive(Debug, Clone)]
pub struct SpecialistPromptContext {
    pub language: String,
    pub additional_system_prompt: Option<String>,
}
