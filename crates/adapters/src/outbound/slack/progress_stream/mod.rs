mod block_builder;
mod chunk_rotation;
mod progress_models;
mod progress_reporter;
mod stream_lifecycle;

use async_trait::async_trait;
use serde_json::{Map, Value};

pub use progress_reporter::SlackProgressReporter;

pub(crate) use block_builder::{build_progress_chunks, build_stream_start_chunks};
pub(crate) use progress_models::{
    SlackAnyChunk, SlackAppendStreamInput, SlackChunkSourceType, SlackMarkdownTextChunk,
    SlackStartStreamInput, SlackStartStreamOutput, SlackStopStreamInput, SlackStreamBlock,
    SlackTaskUpdateChunk, SlackTaskUpdateStatus,
};
pub(crate) use stream_lifecycle::SlackProgressStreamLifecycle;

pub(crate) type SlackProgressLogMeta = Map<String, Value>;

#[async_trait]
pub(crate) trait SlackProgressStreamApiPort: Send + Sync {
    async fn start(
        &self,
        input: SlackStartStreamInput,
    ) -> Result<SlackStartStreamOutput, reili_core::error::PortError>;
    async fn append(
        &self,
        input: SlackAppendStreamInput,
    ) -> Result<(), reili_core::error::PortError>;
    async fn stop(&self, input: SlackStopStreamInput) -> Result<(), reili_core::error::PortError>;
}

pub(crate) trait SlackProgressStreamLogger: Send + Sync {
    fn info(&self, message: &str, meta: SlackProgressLogMeta);
    fn warn(&self, message: &str, meta: SlackProgressLogMeta);
}

#[derive(Debug, Default)]
pub(crate) struct TracingSlackProgressStreamLogger;

impl SlackProgressStreamLogger for TracingSlackProgressStreamLogger {
    fn info(&self, message: &str, meta: SlackProgressLogMeta) {
        let meta_json = serde_json::Value::Object(meta);
        tracing::info!(message = message, meta = %meta_json);
    }

    fn warn(&self, message: &str, meta: SlackProgressLogMeta) {
        let meta_json = serde_json::Value::Object(meta);
        tracing::warn!(message = message, meta = %meta_json);
    }
}

pub(crate) fn string_log_meta<K, V, I>(entries: I) -> SlackProgressLogMeta
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), Value::String(value.into())))
        .collect()
}
