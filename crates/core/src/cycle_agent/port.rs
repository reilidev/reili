use std::sync::Arc;

use async_trait::async_trait;

use super::brief::CycleBrief;
use super::budget::RunBudget;
use crate::error::AgentRunFailedError;
use crate::logger::Logger;
use crate::task::{LlmUsageSnapshot, TaskCancellation, TaskProgressEventPort, TaskResources};
use crate::work::CycleReport;

pub struct RunCycleAgentInput {
    pub brief: CycleBrief,
    pub budget: RunBudget,
    pub resources: TaskResources,
    pub cancellation: TaskCancellation,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub logger: Arc<dyn Logger>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CycleAgentRun {
    /// `None` when the agent stopped without submitting a report.
    pub report: Option<CycleReport>,
    pub usage: LlmUsageSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CycleAgentOutcome {
    Completed(Box<CycleAgentRun>),
    Cancelled,
}

#[async_trait]
pub trait CycleAgentPort: Send + Sync {
    async fn run(
        &self,
        input: RunCycleAgentInput,
    ) -> Result<CycleAgentOutcome, AgentRunFailedError>;
}
