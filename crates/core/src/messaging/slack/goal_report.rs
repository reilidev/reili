use async_trait::async_trait;

use crate::error::PortError;

/// A goal report posted to the goal's Slack channel. `report_key` is attached as Slack message
/// metadata so a retried post after a crash can detect that the report already went out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReportMessage {
    pub channel_id: String,
    pub goal_id: String,
    pub report_key: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedGoalReport {
    pub channel_id: String,
    pub ts: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait GoalReportPort: Send + Sync {
    /// Looks for a recent message in `channel_id` carrying `report_key` in its metadata.
    async fn find_posted_report(
        &self,
        channel_id: &str,
        report_key: &str,
    ) -> Result<Option<PostedGoalReport>, PortError>;

    async fn post_report(&self, message: GoalReportMessage) -> Result<PostedGoalReport, PortError>;

    /// Posts a reply in a thread (used for Slack status commands).
    async fn post_thread_reply(
        &self,
        channel_id: &str,
        thread_ts: &str,
        text: &str,
    ) -> Result<(), PortError>;

    /// Permalink for a posted message, used as the Canvas memory source.
    async fn permalink(&self, channel_id: &str, ts: &str) -> Result<Option<String>, PortError>;
}
