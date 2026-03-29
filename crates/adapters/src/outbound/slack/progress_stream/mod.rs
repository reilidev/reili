mod block_builder;
mod chunk_rotation;
mod progress_models;
mod progress_reporter;
mod stream_lifecycle;

use async_trait::async_trait;
pub(crate) use reili_core::logger::{
    LogFieldValue, Logger as SlackProgressStreamLogger, log_fields as string_log_meta,
};

pub use progress_reporter::{SlackProgressReporter, SlackProgressReporterInput};

pub(crate) use block_builder::build_progress_chunks;
pub(crate) use progress_models::{
    SlackAnyChunk, SlackAppendStreamInput, SlackChunkSourceType, SlackStartStreamInput,
    SlackStartStreamOutput, SlackStopStreamInput, SlackStreamBlock, SlackTaskUpdateChunk,
    SlackTaskUpdateStatus,
};
pub(crate) use stream_lifecycle::SlackProgressStreamLifecycle;

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
