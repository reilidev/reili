use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartInvestigationProgressSessionInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvestigationProgressScopeStatus {
    InProgress,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvestigationProgressUpdate {
    ScopeStarted {
        step_id: String,
        owner_id: String,
        title: String,
        detail: Option<String>,
    },
    ScopeUpdated {
        step_id: String,
        owner_id: String,
        title: String,
        status: InvestigationProgressScopeStatus,
        detail: Option<String>,
    },
    ScopeCompleted {
        step_id: String,
        owner_id: String,
        title: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvestigationProgressSessionCompletionStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteInvestigationProgressSessionInput {
    pub status: InvestigationProgressSessionCompletionStatus,
}

#[async_trait]
pub trait InvestigationProgressSessionPort: Send {
    async fn start(&mut self);
    async fn apply(&mut self, update: InvestigationProgressUpdate);
    async fn complete(&mut self, input: CompleteInvestigationProgressSessionInput);
}

pub trait InvestigationProgressSessionFactoryPort: Send + Sync {
    fn create_for_thread(
        &self,
        input: StartInvestigationProgressSessionInput,
    ) -> Box<dyn InvestigationProgressSessionPort>;
}
