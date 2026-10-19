mod block_builder;
mod chunk_rotation;
mod progress_models;
mod progress_reporter;
mod stream_api_port;
mod stream_lifecycle;

pub(crate) use reili_core::logger::{
    LogFieldValue, Logger as SlackProgressStreamLogger, log_fields as string_log_meta,
};

pub use progress_reporter::{SlackProgressReporter, SlackProgressReporterInput};

pub(crate) use block_builder::build_progress_chunks;
pub(crate) use progress_models::{
    SlackAnyChunk, SlackAppendStreamInput, SlackChunkSourceType, SlackPlanUpdateChunk,
    SlackStartStreamInput, SlackStartStreamOutput, SlackStopStreamInput, SlackStreamBlock,
    SlackTaskUpdateChunk, SlackTaskUpdateStatus,
};
pub(crate) use stream_api_port::SlackProgressStreamApiPort;
pub(crate) use stream_lifecycle::SlackProgressStreamLifecycle;
