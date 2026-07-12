/// A durable Fact/Evidence/Scope note recalled for a task. `source_url` links to the originating
/// thread; `created_at` is an ISO 8601 UTC timestamp. `shared` is true when the note applies across
/// all channels rather than only the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMemoryItem {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    pub source_url: Option<String>,
    pub created_at: String,
    pub shared: bool,
}
