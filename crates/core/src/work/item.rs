//! A work item is an ordinary issue or ticket on the team's tracker. Reili finds its own work the
//! way a teammate does: items labelled for an objective and assigned to Reili. Everything else
//! about the item (title, description, comments, open/closed) is whatever the tracker shows.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::record::RecordRef;
use crate::objective::ObjectiveId;

const MAX_KEY_LENGTH: usize = 80;

/// Stable de-duplication key for a problem within an objective, e.g. `checkout-api:5xx-after-deploy`.
/// Reili stores it as hidden metadata on the items it creates.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkItemKey(String);

impl WorkItemKey {
    /// Normalizes free text into a key: lowercase ASCII alphanumerics separated by single `-`,
    /// keeping `:` and `/` as segment separators. Returns `None` when nothing usable remains.
    #[must_use]
    pub fn normalize(raw: &str) -> Option<Self> {
        let mut key = String::new();
        let mut last_was_dash = true;
        for character in raw.trim().chars().flat_map(char::to_lowercase) {
            if character.is_ascii_alphanumeric() {
                key.push(character);
                last_was_dash = false;
            } else if character == ':' || character == '/' {
                while key.ends_with('-') {
                    key.pop();
                }
                key.push(character);
                last_was_dash = true;
            } else if !last_was_dash {
                key.push('-');
                last_was_dash = true;
            }
            if key.len() >= MAX_KEY_LENGTH {
                break;
            }
        }
        let key = key
            .trim_matches(|character| character == '-' || character == ':' || character == '/')
            .to_string();
        key.chars()
            .any(|character| character.is_ascii_alphanumeric())
            .then_some(Self(key))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for WorkItemKey {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::normalize(&value).ok_or_else(|| format!("invalid work item key `{value}`"))
    }
}

impl From<WorkItemKey> for String {
    fn from(value: WorkItemKey) -> Self {
        value.0
    }
}

/// Where the agent says a work item stands after this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
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

/// A work item as the tracker shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkItem {
    pub record: RecordRef,
    pub objective_id: ObjectiveId,
    pub title: String,
    /// Description, without Reili's hidden metadata.
    pub body: String,
    pub author: String,
    pub created_by_reili: bool,
    /// Present on items Reili created (from its hidden metadata).
    pub key: Option<WorkItemKey>,
    pub state: WorkItemState,
    /// Carries the assignee label: Reili's own work.
    pub assigned_to_reili: bool,
    /// Carries the needs-decision label.
    pub needs_decision: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkItem {
    /// Open and assigned to Reili: what Reili works on.
    #[must_use]
    pub fn is_reili_work(&self) -> bool {
        self.state == WorkItemState::Open && self.assigned_to_reili
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemComment {
    pub author: String,
    /// Written by Reili (its own progress notes).
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
    pub key: WorkItemKey,
    pub title: String,
    pub body: String,
    pub needs_decision: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    Done,
    DuplicateOf(RecordRef),
}

#[cfg(test)]
mod tests {
    use super::WorkItemKey;

    #[test]
    fn normalizes_keys() {
        let key = WorkItemKey::normalize("  Checkout API: 5xx after Deploy! ").expect("key");

        assert_eq!(key.as_str(), "checkout-api:5xx-after-deploy");
        assert!(WorkItemKey::normalize("!!!").is_none());
    }
}
