//! Port for durable Work State, independent of where records are stored. Each backend (GitHub
//! Issues today; ticket systems such as JIRA can be added) keeps one record per objective ledger
//! and per work item in a single, explicitly configured project, and must provide:
//!
//! - a stable, human-readable record key (e.g. `#12`, `OPS-123`) and a URL;
//! - strongly consistent listing by objective, returned in creation order (oldest first);
//! - a revision stored with the state, checked before every write;
//! - a status humans can change (label, workflow status, open/closed) mapped to
//!   [`WorkItemStatus`];
//! - comments with trusted-author detection and an id usable as a "newer than" cursor;
//! - state decoded with [`decode_state_json`](super::schema::decode_state_json), so state from a
//!   newer schema is refused rather than overwritten.
//!
//! `conformance::assert_work_state_contract` (`test-support` feature) checks these guarantees;
//! every backend runs it in its tests.
//!
//! Ledgers and work items share one port on purpose: both are records in the same project and
//! use the same record operations (comments, duplicate closing). Split the port only when a
//! backend needs to keep them in different places.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::feedback::HumanComment;
use super::item::{WorkItemState, WorkItemStatus};
use super::ledger::ObjectiveLedgerState;
use super::record::{CommentId, RecordKey, RecordRef};
use super::schema::StateDecodeError;
use crate::error::PortError;
use crate::objective::ObjectiveId;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredLedger {
    pub record: RecordRef,
    pub state: ObjectiveLedgerState,
    /// Free-form human notes preserved verbatim across Reili re-renders.
    pub human_notes: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredWorkItem {
    pub record: RecordRef,
    pub state: WorkItemState,
    pub human_notes: String,
    /// Status implied by the record's human-editable status in the backend. Differs from
    /// `state.status` when a human changed it (e.g. a label, workflow status, or closing).
    pub observed_status: WorkItemStatus,
    /// Last time anything on the record changed (edits, status, comments).
    pub remote_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkStateError {
    /// Someone else wrote the record since it was read (optimistic concurrency check failed).
    #[error("work state conflict on {record}: expected revision {expected}, found {found}")]
    Conflict {
        record: RecordKey,
        expected: u64,
        found: u64,
    },
    /// The machine-readable state block is missing or unparseable (e.g. edited by hand).
    #[error("work state on {record} is unreadable: {message}")]
    Corrupted { record: RecordKey, message: String },
    /// The state was written by a newer Reili. Never skipped or overwritten: upgrade Reili.
    #[error(
        "work state on {record} has schema version {found}, newer than the supported {supported}; upgrade Reili"
    )]
    UnsupportedSchema {
        record: RecordKey,
        found: u32,
        supported: u32,
    },
    /// The backend could not be reached or returned a server error; safe to retry later.
    #[error("work state backend unavailable: {0}")]
    Unavailable(PortError),
    /// The write was refused by policy (e.g. a record outside the configured project).
    #[error("work state write forbidden: {0}")]
    Forbidden(String),
    #[error("work state error: {0}")]
    Other(PortError),
}

impl WorkStateError {
    #[must_use]
    pub fn undecodable(record: RecordKey, error: StateDecodeError) -> Self {
        match error {
            StateDecodeError::Invalid(message) => Self::Corrupted { record, message },
            StateDecodeError::Unsupported { found, supported } => Self::UnsupportedSchema {
                record,
                found,
                supported,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStateCommentKind {
    CycleLog,
    Decision,
    Notice,
}

impl WorkStateCommentKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CycleLog => "cycle-log",
            Self::Decision => "decision",
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
    /// Reads the ledger for an objective directly (no search index), or `None` if not created yet.
    async fn find_objective_ledger(
        &self,
        objective_id: &ObjectiveId,
    ) -> Result<Option<StoredLedger>, WorkStateError>;

    async fn list_objective_ledgers(&self) -> Result<Vec<StoredLedger>, WorkStateError>;

    async fn create_objective_ledger(
        &self,
        state: &ObjectiveLedgerState,
    ) -> Result<StoredLedger, WorkStateError>;

    /// Re-reads the record and writes `state` with `revision = expected_revision + 1` only if the
    /// stored revision still equals `expected_revision`. This narrows, but cannot eliminate, the
    /// race window on backends without conditional writes (such as GitHub).
    async fn save_objective_ledger(
        &self,
        record: &RecordRef,
        expected_revision: u64,
        state: &ObjectiveLedgerState,
    ) -> Result<StoredLedger, WorkStateError>;

    /// Work items of an objective in creation order (oldest first).
    async fn list_work_items(
        &self,
        objective_id: &ObjectiveId,
    ) -> Result<Vec<StoredWorkItem>, WorkStateError>;

    async fn create_work_item(
        &self,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError>;

    async fn save_work_item(
        &self,
        record: &RecordRef,
        expected_revision: u64,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError>;

    async fn append_comment(
        &self,
        record: &RecordRef,
        comment: WorkStateComment,
    ) -> Result<(), WorkStateError>;

    /// Comments not written by Reili, oldest first, posted after the comment `after_comment_id`
    /// (all comments when `None`).
    async fn list_human_comments(
        &self,
        record: &RecordRef,
        after_comment_id: Option<CommentId>,
    ) -> Result<Vec<HumanComment>, WorkStateError>;

    /// Closes `duplicate` as a duplicate of `canonical` (used after a create race).
    async fn close_duplicate(
        &self,
        duplicate: &RecordRef,
        canonical: &RecordRef,
    ) -> Result<(), WorkStateError>;
}
