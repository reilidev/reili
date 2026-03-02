use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    errors::SynthesizerRunFailedError,
    types::{AlertContext, InvestigationResult, LlmUsageSnapshot},
};

use super::InvestigationProgressEventPort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizerRunReport {
    pub report_text: String,
    pub usage: LlmUsageSnapshot,
}

pub struct RunSynthesizerInput {
    pub result: InvestigationResult,
    pub alert_context: AlertContext,
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
}

#[async_trait]
pub trait InvestigationSynthesizerRunnerPort: Send + Sync {
    async fn run(
        &self,
        input: RunSynthesizerInput,
    ) -> Result<SynthesizerRunReport, SynthesizerRunFailedError>;
}
