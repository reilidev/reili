use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartTaskProgressSessionInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgressScopeStatus {
    InProgress,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgressUpdate {
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
        status: TaskProgressScopeStatus,
        detail: Option<String>,
    },
    ScopeCompleted {
        step_id: String,
        owner_id: String,
        title: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskProgressSessionCompletionStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteTaskProgressSessionInput {
    pub status: TaskProgressSessionCompletionStatus,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait TaskProgressSessionPort: Send {
    async fn start(&mut self);
    async fn apply(&mut self, update: TaskProgressUpdate);
    async fn complete(&mut self, input: CompleteTaskProgressSessionInput);
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
pub trait TaskProgressSessionFactoryPort: Send + Sync {
    fn create_for_thread(
        &self,
        input: StartTaskProgressSessionInput,
    ) -> Box<dyn TaskProgressSessionPort>;
}
