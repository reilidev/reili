use serde::Deserialize;

/// A pointer to where something was observed. Raw logs and payloads are never stored; the
/// reference (URL, query) is enough for a human to reproduce the observation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EvidenceRef {
    pub id: String,
    #[serde(default)]
    pub url: Option<String>,
    pub description: String,
    /// Query or retrieval conditions used to obtain the evidence.
    #[serde(default)]
    pub query: Option<String>,
}
