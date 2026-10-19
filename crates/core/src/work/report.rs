use serde::Deserialize;

use super::evidence::EvidenceRef;
use super::item::WorkItemStatus;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkItemProposal {
    /// Name for the item within this report; `[[key]]` in the Slack message links to it.
    pub key: String,
    /// Record key of the existing work item this is about, as shown in the brief.
    #[serde(default)]
    pub existing_record: Option<String>,
    pub title: String,
    pub status: WorkItemStatus,
    /// Markdown description of a new item, written in the shape the work calls for.
    #[serde(default)]
    pub description: String,
    /// Markdown progress comment on an existing item, written in the shape the work calls for.
    #[serde(default)]
    pub comment: String,
    /// The concrete question for the team when `status` is `needs_decision`.
    #[serde(default)]
    pub decision_needed: Option<String>,
    /// Evidence the text relies on; linked at the end of the description or comment.
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KnowledgeCandidate {
    pub fact: String,
    pub evidence: String,
    pub scope: String,
    #[serde(default)]
    pub shared: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CycleReport {
    /// Slack mrkdwn for the report channel, written for the situation; `None` posts nothing.
    /// `[[key]]` refers to a work item in this report and becomes a link to it.
    #[serde(default)]
    pub slack_message: Option<String>,
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
    use crate::work::WorkItemStatus;

    #[test]
    fn deserializes_minimal_report() {
        let report: CycleReport =
            serde_json::from_str(r#"{"no_new_findings":true}"#).expect("deserialize");

        assert!(report.no_new_findings);
        assert!(report.work_items.is_empty());
    }

    #[test]
    fn deserializes_work_item_proposal() {
        let report: CycleReport = serde_json::from_str(
            r#"{
              "evidence":[{"id":"E1","url":"https://app.datadoghq.com/x","description":"5xx"}],
              "work_items":[{
                "key":"checkout:5xx","title":"5xx",
                "status":"needs_decision","description":"p","comment":"compared windows",
                "evidence_ids":["E1"]
              }]
            }"#,
        )
        .expect("deserialize");

        let item = &report.work_items[0];
        assert_eq!(item.status, WorkItemStatus::NeedsDecision);
        assert!(report.evidence_by_id("E1").is_some());
    }
}
