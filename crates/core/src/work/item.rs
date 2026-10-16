//! A Work Item is one durable unit of continuing work (a risk, incident pattern, or improvement
//! opportunity) persisted as a GitHub Issue in the management repository.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::investigation::{EvidenceRef, Finding, Hypothesis, Recommendation};
use crate::goal::GoalId;

pub const WORK_ITEM_SCHEMA_VERSION: u32 = 1;
const MAX_KEY_LENGTH: usize = 80;
const MAX_HYPOTHESES: usize = 10;
const MAX_FINDINGS: usize = 10;
const MAX_EVIDENCE: usize = 30;
const MAX_FEEDBACK: usize = 20;
const MAX_HISTORY: usize = 30;
const MAX_LIST_ENTRIES: usize = 10;

/// Stable de-duplication key for a problem within a goal, e.g. `checkout-api:5xx-after-deploy`.
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

    /// Token-set similarity used to catch near-duplicate keys the model produced in different
    /// cycles (e.g. `checkout:5xx-after-deploy` vs `checkout:deploy-5xx`).
    #[must_use]
    pub fn similarity(&self, other: &Self) -> f64 {
        token_jaccard(self.as_str(), other.as_str())
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

/// Jaccard similarity over lowercase alphanumeric tokens.
#[must_use]
pub fn token_jaccard(left: &str, right: &str) -> f64 {
    let tokens = |value: &str| -> std::collections::BTreeSet<String> {
        value
            .to_lowercase()
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(ToString::to_string)
            .collect()
    };
    let left = tokens(left);
    let right = tokens(right);
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection = left.intersection(&right).count();
    let union = left.union(&right).count();
    #[allow(clippy::cast_precision_loss)]
    let similarity = intersection as f64 / union as f64;
    similarity
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemStatus {
    Investigating,
    NeedsHuman,
    Proposed,
    Resolved,
    Dismissed,
}

impl WorkItemStatus {
    pub const ALL: [Self; 5] = [
        Self::Investigating,
        Self::NeedsHuman,
        Self::Proposed,
        Self::Resolved,
        Self::Dismissed,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Investigating => "investigating",
            Self::NeedsHuman => "needs-human",
            Self::Proposed => "proposed",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == normalized)
    }

    #[must_use]
    pub fn is_open(self) -> bool {
        !matches!(self, Self::Resolved | Self::Dismissed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemKind {
    Risk,
    Incident,
    Improvement,
    Toil,
    Security,
    Cost,
}

impl WorkItemKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Risk => "risk",
            Self::Incident => "incident",
            Self::Improvement => "improvement",
            Self::Toil => "toil",
            Self::Security => "security",
            Self::Cost => "cost",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusActor {
    Reili,
    Human { login: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusChange {
    pub at: DateTime<Utc>,
    pub from: Option<WorkItemStatus>,
    pub to: WorkItemStatus,
    pub actor: StatusActor,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackNote {
    pub comment_id: u64,
    pub author: String,
    pub at: DateTime<Utc>,
    pub text: String,
    /// Set once a later cycle has received the note in its brief.
    #[serde(default)]
    pub applied_in_run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanDecisionRequest {
    pub question: String,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationState {
    #[serde(default)]
    pub last_digest: Option<String>,
    #[serde(default)]
    pub last_notified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItemState {
    pub schema_version: u32,
    pub key: WorkItemKey,
    pub goal_id: GoalId,
    pub title: String,
    pub kind: WorkItemKind,
    pub severity: Severity,
    pub status: WorkItemStatus,
    pub problem_statement: String,
    #[serde(default)]
    pub plan: Vec<String>,
    #[serde(default)]
    pub hypotheses: Vec<Hypothesis>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub recommendation: Option<Recommendation>,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub human_decision: Option<HumanDecisionRequest>,
    #[serde(default)]
    pub next_actions: Vec<String>,
    #[serde(default)]
    pub feedback: Vec<FeedbackNote>,
    #[serde(default)]
    pub history: Vec<StatusChange>,
    #[serde(default)]
    pub last_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_processed_comment_id: Option<u64>,
    #[serde(default)]
    pub notification: NotificationState,
    #[serde(default)]
    pub revision: u64,
}

impl WorkItemState {
    /// Records a status transition in the item history (no-op when the status is unchanged).
    pub fn transition(
        &mut self,
        to: WorkItemStatus,
        actor: StatusActor,
        reason: impl Into<String>,
        at: DateTime<Utc>,
    ) {
        if self.status == to {
            return;
        }
        self.history.push(StatusChange {
            at,
            from: Some(self.status),
            to,
            actor,
            reason: reason.into(),
        });
        self.status = to;
        self.updated_at = at;
    }

    /// Keeps persisted state bounded so it always fits in a GitHub issue body. The newest
    /// entries win; old evidence remains reachable through issue comments.
    pub fn enforce_limits(&mut self) {
        keep_last(&mut self.hypotheses, MAX_HYPOTHESES);
        keep_last(&mut self.findings, MAX_FINDINGS);
        keep_last(&mut self.evidence, MAX_EVIDENCE);
        keep_last(&mut self.feedback, MAX_FEEDBACK);
        keep_last(&mut self.history, MAX_HISTORY);
        keep_last(&mut self.plan, MAX_LIST_ENTRIES);
        keep_last(&mut self.open_questions, MAX_LIST_ENTRIES);
        keep_last(&mut self.next_actions, MAX_LIST_ENTRIES);
    }

    #[must_use]
    pub fn pending_feedback(&self) -> Vec<&FeedbackNote> {
        self.feedback
            .iter()
            .filter(|note| note.applied_in_run.is_none())
            .collect()
    }
}

pub(crate) fn keep_last<T>(values: &mut Vec<T>, max: usize) {
    if values.len() > max {
        let excess = values.len() - max;
        values.drain(..excess);
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{StatusActor, WorkItemKey, WorkItemStatus, token_jaccard};
    use crate::work::test_support::sample_work_item;

    #[test]
    fn normalizes_keys() {
        assert_eq!(
            WorkItemKey::normalize("Checkout-API: 5xx after Deploy!")
                .expect("key")
                .as_str(),
            "checkout-api:5xx-after-deploy"
        );
        assert_eq!(
            WorkItemKey::normalize("  db / slow   queries ")
                .expect("key")
                .as_str(),
            "db/slow-queries"
        );
        assert!(WorkItemKey::normalize("!!!").is_none());
        assert!(
            WorkItemKey::normalize(&"a".repeat(200))
                .expect("key")
                .as_str()
                .len()
                <= 80
        );
    }

    #[test]
    fn similarity_detects_reordered_tokens() {
        let left = WorkItemKey::normalize("checkout:5xx-after-deploy").expect("key");
        let right = WorkItemKey::normalize("checkout:deploy-5xx-after").expect("key");

        assert!((left.similarity(&right) - 1.0).abs() < f64::EPSILON);
        assert!(token_jaccard("a b c", "x y z") < 0.01);
    }

    #[test]
    fn parses_status_labels() {
        assert_eq!(
            WorkItemStatus::parse("needs_human"),
            Some(WorkItemStatus::NeedsHuman)
        );
        assert_eq!(WorkItemStatus::parse("unknown"), None);
        assert!(!WorkItemStatus::Dismissed.is_open());
    }

    #[test]
    fn transition_records_history_once() {
        let mut item = sample_work_item("checkout:errors");
        let at = Utc.timestamp_opt(1_700_000_000, 0).single().expect("time");

        item.transition(WorkItemStatus::NeedsHuman, StatusActor::Reili, "decide", at);
        item.transition(WorkItemStatus::NeedsHuman, StatusActor::Reili, "again", at);

        assert_eq!(item.history.len(), 1);
        assert_eq!(item.status, WorkItemStatus::NeedsHuman);
    }

    #[test]
    fn enforces_bounded_state() {
        let mut item = sample_work_item("checkout:errors");
        item.open_questions = (0..50).map(|index| format!("q{index}")).collect();

        item.enforce_limits();

        assert_eq!(item.open_questions.len(), 10);
        assert_eq!(item.open_questions[0], "q40");
    }
}
