use reili_core::work::{CycleReport, EvidenceRef, WorkItemProposal, WorkItemStatus};

pub(crate) use reili_core::work::test_support::sample_objective;

pub(crate) fn proposal() -> WorkItemProposal {
    WorkItemProposal {
        key: "checkout:5xx-after-deploy".to_string(),
        existing_record: None,
        title: "5xx after deploy".to_string(),
        status: WorkItemStatus::NeedsDecision,
        description: "5xx rate doubled".to_string(),
        comment: "Compared the 5xx rate before and after deploy 42.".to_string(),
        decision_needed: Some("Roll back deploy 42?".to_string()),
        evidence_ids: vec!["E1".to_string()],
    }
}

pub(crate) fn report(work_items: Vec<WorkItemProposal>) -> CycleReport {
    CycleReport {
        slack_message: None,
        evidence: vec![EvidenceRef {
            id: "E1".to_string(),
            url: Some("https://app.datadoghq.com/apm/checkout".to_string()),
            description: "5xx dashboard".to_string(),
            query: None,
        }],
        work_items,
        knowledge_candidates: Vec::new(),
        no_new_findings: false,
    }
}
