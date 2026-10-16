//! The structured report a goal investigation submits. Reili's report policy screens it before
//! anything is recorded.

use serde::{Deserialize, Serialize};

use super::investigation::{EvidenceRef, Finding, Hypothesis, Recommendation};
use super::item::{Severity, WorkItemKind, WorkItemStatus};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItemProposal {
    /// Stable de-duplication key; reuse the key of an existing work item for the same problem.
    pub key: String,
    #[serde(default)]
    pub existing_issue_number: Option<u64>,
    pub title: String,
    pub kind: WorkItemKind,
    pub severity: Severity,
    pub proposed_status: WorkItemStatus,
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
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub human_decision_needed: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvestigationReport {
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

impl InvestigationReport {
    #[must_use]
    pub fn evidence_by_id(&self, id: &str) -> Option<&EvidenceRef> {
        self.evidence.iter().find(|evidence| evidence.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::InvestigationReport;
    use crate::work::{Severity, WorkItemStatus};

    #[test]
    fn deserializes_minimal_report() {
        let report: InvestigationReport =
            serde_json::from_str(r#"{"summary":"nothing new","no_new_findings":true}"#)
                .expect("deserialize");

        assert!(report.no_new_findings);
        assert!(report.work_items.is_empty());
    }

    #[test]
    fn deserializes_work_item_proposal() {
        let report: InvestigationReport = serde_json::from_str(
            r#"{
              "summary":"s",
              "evidence":[{"id":"E1","source":"datadog","url":"https://app.datadoghq.com/x","description":"5xx"}],
              "work_items":[{
                "key":"checkout:5xx","title":"5xx","kind":"incident","severity":"high",
                "proposed_status":"needs_human","problem_statement":"p",
                "hypotheses":[{"id":"H1","statement":"deploy","status":"supported"}]
              }]
            }"#,
        )
        .expect("deserialize");

        let item = &report.work_items[0];
        assert_eq!(item.severity, Severity::High);
        assert_eq!(item.proposed_status, WorkItemStatus::NeedsHuman);
        assert!(report.evidence_by_id("E1").is_some());
    }
}
