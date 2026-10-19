use std::fmt;

/// Stable, human-readable key a backend assigns to a record (e.g. `#12` for GitHub, `OPS-123`
/// for JIRA). Shown to the agent and humans, and used to refer to existing work items.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordKey(String);

impl RecordKey {
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RecordKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRef {
    pub key: RecordKey,
    pub url: String,
}
