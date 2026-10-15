//! Port for durable Work State. The production adapter stores it in GitHub Issues of a single,
//! explicitly configured management repository.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::feedback::HumanComment;
use super::item::{WorkItemState, WorkItemStatus};
use super::ledger::GoalLedgerState;
use crate::error::PortError;
use crate::goal::GoalId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRef {
    pub number: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredLedger {
    pub issue: IssueRef,
    pub state: GoalLedgerState,
    /// Free-form human notes preserved verbatim across Reili re-renders.
    pub human_notes: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredWorkItem {
    pub issue: IssueRef,
    pub state: WorkItemState,
    pub human_notes: String,
    /// Status implied by the issue's current labels and open/closed state. Differs from
    /// `state.status` when a human changed the label or closed/reopened the issue.
    pub observed_status: WorkItemStatus,
    /// Last time anything on the issue changed (edits, labels, comments).
    pub remote_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkStateError {
    /// Someone else wrote the record since it was read (optimistic concurrency check failed).
    #[error("work state conflict on #{issue_number}: expected revision {expected}, found {found}")]
    Conflict {
        issue_number: u64,
        expected: u64,
        found: u64,
    },
    /// The machine-readable state block is missing or unparseable (e.g. edited by hand).
    #[error("work state on #{issue_number} is unreadable: {message}")]
    Corrupted { issue_number: u64, message: String },
    /// The backend could not be reached or returned a server error; safe to retry later.
    #[error("work state backend unavailable: {0}")]
    Unavailable(PortError),
    /// The write was refused by policy (e.g. a repository outside the allowlist).
    #[error("work state write forbidden: {0}")]
    Forbidden(String),
    #[error("work state error: {0}")]
    Other(PortError),
}

impl WorkStateError {
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Conflict { .. } | Self::Unavailable(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStateCommentKind {
    CycleLog,
    Decision,
    Outcome,
    Notice,
}

impl WorkStateCommentKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CycleLog => "cycle-log",
            Self::Decision => "decision",
            Self::Outcome => "outcome",
            Self::Notice => "notice",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkStateComment {
    pub kind: WorkStateCommentKind,
    pub body: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait WorkStateRepositoryPort: Send + Sync {
    /// Reads the ledger for a goal directly (no search index), or `None` if not created yet.
    async fn find_goal_ledger(
        &self,
        goal_id: &GoalId,
    ) -> Result<Option<StoredLedger>, WorkStateError>;

    async fn list_goal_ledgers(&self) -> Result<Vec<StoredLedger>, WorkStateError>;

    async fn create_goal_ledger(
        &self,
        state: &GoalLedgerState,
    ) -> Result<StoredLedger, WorkStateError>;

    /// Re-reads the record and writes `state` with `revision = expected_revision + 1` only if the
    /// stored revision still equals `expected_revision`. This narrows, but cannot eliminate, the
    /// race window: GitHub offers no conditional writes.
    async fn save_goal_ledger(
        &self,
        issue: &IssueRef,
        expected_revision: u64,
        state: &GoalLedgerState,
    ) -> Result<StoredLedger, WorkStateError>;

    async fn list_work_items(
        &self,
        goal_id: &GoalId,
    ) -> Result<Vec<StoredWorkItem>, WorkStateError>;

    async fn create_work_item(
        &self,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError>;

    async fn save_work_item(
        &self,
        issue: &IssueRef,
        expected_revision: u64,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError>;

    async fn append_comment(
        &self,
        issue: &IssueRef,
        comment: WorkStateComment,
    ) -> Result<(), WorkStateError>;

    /// Comments not written by Reili, oldest first, with ids greater than `after_comment_id`.
    async fn list_human_comments(
        &self,
        issue: &IssueRef,
        after_comment_id: Option<u64>,
    ) -> Result<Vec<HumanComment>, WorkStateError>;

    /// Closes `duplicate` as a duplicate of `canonical` (used after a create race).
    async fn close_duplicate(
        &self,
        duplicate: &IssueRef,
        canonical: &IssueRef,
    ) -> Result<(), WorkStateError>;
}
