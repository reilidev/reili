use async_trait::async_trait;

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoResponseJudgeInput {
    /// Channel-specific judge policy; the judge falls back to its built-in
    /// policy when omitted.
    pub policy: Option<String>,
    pub message_text: String,
    pub thread_context: Vec<AutoResponseContextMessage>,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoResponseContextMessage {
    pub ts: String,
    pub user: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoResponseJudgeDecision {
    pub respond: bool,
    pub reason: Option<String>,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait AutoResponseJudgePort: Send + Sync {
    async fn judge(
        &self,
        input: AutoResponseJudgeInput,
    ) -> Result<AutoResponseJudgeDecision, PortError>;
}
