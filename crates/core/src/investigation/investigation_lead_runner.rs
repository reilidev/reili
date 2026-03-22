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
pub struct InvestigationLeadRunReport {
    pub result_text: String,
    pub usage: LlmUsageSnapshot,
    pub execution: LlmExecutionMetadata,
}

pub struct RunInvestigationLeadInput {
    pub alert_context: AlertContext,
    pub context: InvestigationContext,
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait InvestigationLeadRunnerPort: Send + Sync {
    async fn run(
        &self,
        input: RunInvestigationLeadInput,
    ) -> Result<InvestigationLeadRunReport, AgentRunFailedError>;
}
