use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use super::item::{NewWorkItem, WorkItem, WorkItemComment};
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

#[async_trait]
pub trait WorkStatePort: Send + Sync {
    /// Items labelled for the objective: every open one plus those closed at or after
    /// `closed_since`, oldest first.
    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
        closed_since: DateTime<Utc>,
    ) -> Result<Vec<WorkItem>, WorkStateError>;

    /// Open items assigned to Reili that carry no objective label, oldest first.
    async fn list_unlinked_reili_work(&self) -> Result<Vec<RecordRef>, WorkStateError>;

    /// Labels an item for the objective. Other labels stay as they are.
    async fn link_to_objective(
        &self,
        record: &RecordRef,
        objective_id: &ObjectiveId,
    ) -> Result<(), WorkStateError>;

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
    async fn close_work_item(&self, record: &RecordRef) -> Result<(), WorkStateError>;
}
