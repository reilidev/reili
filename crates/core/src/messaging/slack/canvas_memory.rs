use async_trait::async_trait;

use crate::error::PortError;

/// Scope a memory is stored under: tied to a single channel, or shared across every channel this
/// Reili instance serves (recalled in all of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackCanvasMemoryVisibility {
    Channel,
    Shared,
}

/// A durable Fact/Evidence/Scope note recalled for a channel. `created_at` is an ISO 8601 UTC
/// timestamp; callers rely on it for newest-first ordering. `visibility` tells whether the note is
/// channel-scoped or shared across all channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackCanvasMemoryRecord {
    pub visibility: SlackCanvasMemoryVisibility,
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    pub source_url: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSlackCanvasMemoriesInput {
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendSlackCanvasMemoryInput {
    /// Section the memory is written to.
    pub visibility: SlackCanvasMemoryVisibility,
    pub channel_id: String,
    pub channel_name: Option<String>,
    /// Originating thread timestamp, resolved to the note's Source permalink.
    pub source_message_ts: String,
    pub fact: String,
    pub evidence: String,
    pub scope: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait SlackCanvasMemoryPort: Send + Sync {
    /// Returns the shared memories plus the channel's own memories, newest-first within each group
    /// and capped at `limit` per group.
    async fn list_channel_memories(
        &self,
        input: ListSlackCanvasMemoriesInput,
    ) -> Result<Vec<SlackCanvasMemoryRecord>, PortError>;

    /// Appends a memory to the section named by `input.visibility`, pruning that section's entries
    /// beyond the retention cap in the same operation.
    async fn append_memory(&self, input: AppendSlackCanvasMemoryInput) -> Result<(), PortError>;
}
