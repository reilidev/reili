/// A single line contributed by a connector to the lead prompt's "Current context" block.
///
/// Rendered as `- {label}: {value}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorPromptFact {
    pub label: String,
    pub value: String,
}
