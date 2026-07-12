/// One selectable tool in the dynamic sub-agent catalog: the exact tool name and a one-line
/// summary shown to the lead agent when it composes a spawned sub-agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub summary: String,
}

impl ToolCatalogEntry {
    #[must_use]
    pub fn new(name: &str, summary: &str) -> Self {
        Self {
            name: name.to_string(),
            summary: summary.to_string(),
        }
    }
}

/// Catalog entries from one tool source, labeled for grouping in the lead prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCatalogGroup {
    pub source: String,
    pub entries: Vec<ToolCatalogEntry>,
}

impl ToolCatalogGroup {
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }
}
