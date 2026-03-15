use std::sync::Arc;

use async_trait::async_trait;

use crate::error::AgentRunFailedError;

use super::{AlertContext, InvestigationContext, InvestigationProgressEventPort, LlmUsageSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmExecutionMetadata {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorRunReport {
    pub result_text: String,
    pub usage: LlmUsageSnapshot,
    pub execution: LlmExecutionMetadata,
}

pub struct RunCoordinatorInput {
    pub alert_context: AlertContext,
    pub context: InvestigationContext,
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
}

#[async_trait]
pub trait InvestigationCoordinatorRunnerPort: Send + Sync {
    async fn run(
        &self,
        input: RunCoordinatorInput,
    ) -> Result<CoordinatorRunReport, AgentRunFailedError>;
}
