//! Port to the team's tracker (GitHub Issues today; ticket systems such as JIRA can be added).
//! Reili keeps no state of its own there: an objective's work is the set of items labelled for it,
//! Reili's own work is the subset carrying the assignee label, and progress lives in comments, as
//! for any teammate. A backend must provide, inside one explicitly configured project:
//!
//! - an objective label, an assignee label for Reili, and a needs-decision label;
//! - a stable, human-readable record key (e.g. `#12`, `OPS-123`) and a URL;
//! - listing by objective label in creation order (oldest first);
//! - hidden metadata on the items Reili creates (its de-duplication key);
//! - comments with detection of Reili's own comments and of trusted authors;
//! - refusal of any record outside the configured project.
//!
//! `conformance::assert_work_state_contract` (`test-support` feature) checks these guarantees;
//! every backend runs it in its tests.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use super::item::{CloseReason, NewWorkItem, WorkItem, WorkItemComment};
use super::record::RecordRef;
use crate::error::PortError;
use crate::objective::ObjectiveId;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkStateError {
    /// The tracker could not be reached or returned a server error; safe to retry later.
    #[error("work tracker unavailable: {0}")]
    Unavailable(PortError),
    /// Refused by policy (e.g. a record outside the configured project).
    #[error("work tracker write forbidden: {0}")]
    Forbidden(String),
    #[error("work tracker error: {0}")]
    Other(PortError),
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait WorkStatePort: Send + Sync {
    /// Items labelled for the objective: every open one plus those closed at or after
    /// `closed_since`, oldest first.
    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
        closed_since: DateTime<Utc>,
    ) -> Result<Vec<WorkItem>, WorkStateError>;

    /// Comments on an item, oldest first.
    async fn list_comments(
        &self,
        record: &RecordRef,
    ) -> Result<Vec<WorkItemComment>, WorkStateError>;

    /// Opens an item labelled for its objective and assigned to Reili.
    async fn create_work_item(&self, item: &NewWorkItem) -> Result<WorkItem, WorkStateError>;

    async fn add_comment(&self, record: &RecordRef, body: &str) -> Result<(), WorkStateError>;

    async fn set_needs_decision(
        &self,
        record: &RecordRef,
        needs_decision: bool,
    ) -> Result<(), WorkStateError>;

    /// Closes an item. Reili only closes items it created.
    async fn close_work_item(
        &self,
        record: &RecordRef,
        reason: &CloseReason,
    ) -> Result<(), WorkStateError>;
}
