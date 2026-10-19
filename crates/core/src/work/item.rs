use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::record::RecordRef;
use crate::objective::ObjectiveId;

/// Where the agent says a work item stands after this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemStatus {
    InProgress,
    /// Waiting for a decision from the team; marked on the tracker with the needs-decision label.
    NeedsDecision,
    Resolved,
}

impl WorkItemStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in-progress",
            Self::NeedsDecision => "needs-decision",
            Self::Resolved => "resolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemState {
    Open,
    /// Closed as done (GitHub: completed).
    Done,
    /// Closed without doing it (GitHub: not planned, duplicate).
    NotPlanned,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkItem {
    pub record: RecordRef,
    pub objective_id: ObjectiveId,
    pub title: String,
    pub body: String,
    pub author: String,
    pub created_by_reili: bool,
    pub state: WorkItemState,
    /// Carries the assignee label: Reili's own work.
    pub assigned_to_reili: bool,
    pub needs_decision: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkItem {
    #[must_use]
    pub fn is_reili_work(&self) -> bool {
        self.state == WorkItemState::Open && self.assigned_to_reili
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemComment {
    pub author: String,
    pub by_reili: bool,
    /// The author is a trusted member of the tracker project (for GitHub: an owner, member, or
    /// collaborator of the repository). Only trusted comments reach the agent.
    pub author_trusted: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

/// A work item Reili opens: labelled for its objective and assigned to Reili.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkItem {
    pub objective_id: ObjectiveId,
    pub title: String,
    pub body: String,
    pub needs_decision: bool,
}
