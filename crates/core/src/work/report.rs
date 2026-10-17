//! The structured report the agent submits at the end of an objective cycle. Reili's report policy
//! screens it before anything is written to the tracker.

use serde::{Deserialize, Serialize};

use super::evidence::{EvidenceRef, Finding, Hypothesis, Recommendation};
use super::item::{Severity, WorkItemStatus};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItemProposal {
    /// Stable de-duplication key; reuse the key of an existing work item for the same problem.
    pub key: String,
    /// Record key of the existing work item this is about, as shown in the brief.
    #[serde(default)]
    pub existing_record: Option<String>,
    pub title: String,
    pub severity: Severity,
    pub status: WorkItemStatus,
    /// What the problem or task is; becomes the description of a new item.
    #[serde(default)]
    pub problem_statement: String,
    /// What was done on this item in this cycle; becomes the progress comment.
    #[serde(default)]
    pub work_log: String,
    #[serde(default)]
    pub hypotheses: Vec<Hypothesis>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub recommendation: Option<Recommendation>,
    /// The concrete question for the team when `status` is `needs_decision`.
    #[serde(default)]
    pub decision_needed: Option<String>,
    #[serde(default)]
    pub next_actions: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCandidate {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub shared: bool,
}

/// A knowledge candidate that passed screening, ready for the Canvas memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeDraft {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    pub shared: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleReport {
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<EvidenceRef>,
    #[serde(default)]
    pub work_items: Vec<WorkItemProposal>,
    #[serde(default)]
    pub knowledge_candidates: Vec<KnowledgeCandidate>,
    /// True when the cycle found nothing new worth recording. This is a valid, expected outcome.
    #[serde(default)]
    pub no_new_findings: bool,
}

impl CycleReport {
    #[must_use]
    pub fn evidence_by_id(&self, id: &str) -> Option<&EvidenceRef> {
        self.evidence.iter().find(|evidence| evidence.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::CycleReport;
    use crate::work::{Severity, WorkItemStatus};

    #[test]
    fn deserializes_minimal_report() {
        let report: CycleReport =
            serde_json::from_str(r#"{"summary":"nothing new","no_new_findings":true}"#)
                .expect("deserialize");

        assert!(report.no_new_findings);
        assert!(report.work_items.is_empty());
    }

    #[test]
    fn deserializes_work_item_proposal() {
        let report: CycleReport = serde_json::from_str(
            r#"{
              "summary":"s",
              "evidence":[{"id":"E1","source":"datadog","url":"https://app.datadoghq.com/x","description":"5xx"}],
              "work_items":[{
                "key":"checkout:5xx","title":"5xx","severity":"high",
                "status":"needs_decision","problem_statement":"p","work_log":"compared windows",
                "hypotheses":[{"id":"H1","statement":"deploy","status":"supported"}]
              }]
            }"#,
        )
        .expect("deserialize");

        let item = &report.work_items[0];
        assert_eq!(item.severity, Severity::High);
        assert_eq!(item.status, WorkItemStatus::NeedsDecision);
        assert!(report.evidence_by_id("E1").is_some());
    }
}
