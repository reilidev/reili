use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    errors::AgentRunFailedError,
    types::{AlertContext, InvestigationResult, LlmUsageSnapshot},
};

use super::{InvestigationContext, InvestigationProgressEventPort};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorRunReport {
    pub result_text: InvestigationResult,
    pub usage: LlmUsageSnapshot,
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
