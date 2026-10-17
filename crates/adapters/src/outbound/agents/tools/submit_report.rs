//! The lead agent's only way to hand results back in objective mode: a schema-validated structured
//! report. Validation errors are returned to the model so it can fix and resubmit.

use std::collections::BTreeSet;

use reili_core::cycle_agent::ReportSlot;
use reili_core::error::PortError;
use reili_core::work::CycleReport;
use rig::tool::{Tool, ToolContext};
use serde_json::{Value, json};

const MAX_WORK_ITEMS: usize = 10;

#[derive(Clone)]
pub struct SubmitCycleReportTool {
    slot: ReportSlot,
}

impl SubmitCycleReportTool {
    #[must_use]
    pub fn new(slot: ReportSlot) -> Self {
        Self { slot }
    }

    fn validate(&self, report: &CycleReport) -> Vec<String> {
        let mut problems = Vec::new();
        if report.summary.trim().is_empty() {
            problems.push("`summary` must not be empty".to_string());
        }
        if report.work_items.len() > MAX_WORK_ITEMS {
            problems.push(format!("at most {MAX_WORK_ITEMS} work items per report"));
        }
        if report.no_new_findings && !report.work_items.is_empty() {
            problems.push(
                "`no_new_findings` is true but `work_items` is not empty; pick one".to_string(),
            );
        }
        let evidence_ids: BTreeSet<&str> = report
            .evidence
            .iter()
            .map(|evidence| evidence.id.as_str())
            .collect();
        if evidence_ids.len() != report.evidence.len() {
            problems.push("evidence ids must be unique".to_string());
        }
        let mut keys = BTreeSet::new();
        for item in &report.work_items {
            if item.key.trim().is_empty() || !keys.insert(item.key.trim().to_lowercase()) {
                problems.push(format!(
                    "work item `{}` needs a unique, non-empty `key`",
                    item.title
                ));
            }
            if item.title.trim().is_empty() || item.problem_statement.trim().is_empty() {
                problems.push(format!(
                    "work item `{}` needs a title and a problem_statement",
                    item.key
                ));
            }
            let cited_evidence = item
                .evidence_ids
                .iter()
                .chain(
                    item.hypotheses
                        .iter()
                        .flat_map(|hypothesis| hypothesis.evidence_ids.iter()),
                )
                .chain(
                    item.findings
                        .iter()
                        .flat_map(|finding| finding.evidence_ids.iter()),
                );
            for id in cited_evidence {
                if !evidence_ids.contains(id.as_str()) {
                    problems.push(format!(
                        "work item `{}` cites evidence `{id}` that is not in `evidence`",
                        item.key
                    ));
                }
            }
        }
        problems
    }
}

impl Tool for SubmitCycleReportTool {
    const NAME: &'static str = "submit_cycle_report";

    type Error = PortError;
    type Args = Value;
    type Output = String;

    fn description(&self) -> String {
        "Submit the final structured report for this objective cycle. Call exactly once, at the end. Reili screens it with its report policy and persists it to the Work State backend: cited evidence ids must exist in `evidence`, and `dismissed` is set only by humans.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "2-4 sentence summary of what changed since the last cycle and what matters." },
                "no_new_findings": { "type": "boolean", "description": "true when nothing new was found; then work_items must be empty." },
                "evidence": {
                    "type": "array",
                    "description": "References only (URL, query, window); never raw logs or secrets.",
                    "items": evidence_schema()
                },
                "work_items": { "type": "array", "items": work_item_schema() },
                "knowledge_candidates": {
                    "type": "array",
                    "description": "Durable, reusable facts confirmed from primary sources (code, config, runbooks). Never hypotheses or time-bounded observations.",
                    "items": knowledge_schema()
                }
            },
            "required": ["summary", "no_new_findings"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let report: CycleReport = serde_json::from_value(args).map_err(|error| {
            PortError::invalid_input(format!(
                "report does not match the schema: {error}. Fix it and call submit_cycle_report again."
            ))
        })?;
        let problems = self.validate(&report);
        if !problems.is_empty() {
            return Err(PortError::invalid_input(format!(
                "report rejected: {}. Fix these and call submit_cycle_report again.",
                problems.join("; ")
            )));
        }
        self.slot.submit(report);
        Ok(
            "Report accepted. Do not call any more tools; reply with a one-line confirmation."
                .to_string(),
        )
    }
}

fn id_list() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn evidence_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "E1, E2, ..." },
            "source": { "type": "string", "enum": ["datadog", "github", "slack", "canvas", "web", "jira", "esa", "metric", "human", "other"] },
            "url": { "type": "string" },
            "description": { "type": "string" },
            "observed_at": { "type": "string", "description": "RFC 3339" },
            "time_range": { "type": "object", "properties": { "from": { "type": "string" }, "to": { "type": "string" } } },
            "query": { "type": "string" }
        },
        "required": ["id", "source", "description"]
    })
}

fn hypothesis_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "statement": { "type": "string" },
            "status": { "type": "string", "enum": ["proposed", "supported", "refuted", "inconclusive"] },
            "evidence_ids": id_list(),
            "rationale": { "type": "string" }
        },
        "required": ["id", "statement", "status"]
    })
}

fn recommendation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string" },
            "rationale": { "type": "string" },
            "expected_effect": { "type": "string" },
            "risk": { "type": "string" }
        },
        "required": ["action"]
    })
}

fn work_item_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Stable dedupe key such as `checkout-api:db-pool-timeouts`; reuse the key of an existing work item for the same problem." },
            "existing_record": { "type": "string", "description": "Record key of the existing work item for this problem, exactly as shown in the brief (e.g. `#12`)." },
            "title": { "type": "string" },
            "kind": { "type": "string", "enum": ["risk", "incident", "improvement", "toil", "security", "cost"] },
            "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] },
            "proposed_status": { "type": "string", "enum": ["in_progress", "needs_human", "proposed", "resolved"] },
            "problem_statement": { "type": "string" },
            "plan": { "type": "array", "items": { "type": "string" } },
            "hypotheses": { "type": "array", "items": hypothesis_schema() },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "statement": { "type": "string" },
                        "evidence_ids": id_list()
                    },
                    "required": ["statement"]
                }
            },
            "recommendation": recommendation_schema(),
            "open_questions": { "type": "array", "items": { "type": "string" } },
            "human_decision_needed": { "type": "string", "description": "The concrete question for humans when proposed_status is needs_human." },
            "next_actions": { "type": "array", "items": { "type": "string" } },
            "evidence_ids": id_list()
        },
        "required": ["key", "title", "kind", "severity", "proposed_status", "problem_statement"]
    })
}

fn knowledge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "fact": { "type": "string" },
            "evidence": { "type": "string" },
            "scope": { "type": "string" },
            "evidence_ids": id_list(),
            "shared": { "type": "boolean" }
        },
        "required": ["fact", "evidence", "scope", "evidence_ids"]
    })
}

#[cfg(test)]
mod tests {
    use reili_core::cycle_agent::ReportSlot;
    use rig::tool::{Tool, ToolContext};
    use serde_json::json;

    use super::SubmitCycleReportTool;

    fn item(evidence: &str) -> serde_json::Value {
        json!({
            "key": "checkout:errors",
            "title": "errors",
            "kind": "incident",
            "severity": "high",
            "proposed_status": "in_progress",
            "problem_statement": "p",
            "hypotheses": [{ "id": "H1", "statement": "s", "status": "supported", "evidence_ids": [evidence] }]
        })
    }

    fn tool() -> (SubmitCycleReportTool, ReportSlot) {
        let slot = ReportSlot::new();
        (SubmitCycleReportTool::new(slot.clone()), slot)
    }

    #[tokio::test]
    async fn accepts_valid_report() {
        let (tool, slot) = tool();

        tool.call(&mut ToolContext::new(), json!({
            "summary": "s",
            "no_new_findings": false,
            "evidence": [{ "id": "E1", "source": "datadog", "url": "https://x", "description": "d" }],
            "work_items": [item("E1")]
        }))
        .await
        .expect("accepted");

        assert_eq!(slot.take().expect("report").work_items.len(), 1);
    }

    #[tokio::test]
    async fn rejects_unknown_references_and_schema_errors() {
        let (tool, slot) = tool();

        let error = tool
            .call(
                &mut ToolContext::new(),
                json!({
                    "summary": "s",
                    "no_new_findings": false,
                    "evidence": [],
                    "work_items": [item("E9")]
                }),
            )
            .await
            .expect_err("rejected");
        assert!(error.message.contains("evidence `E9`"));

        let schema_error = tool
            .call(&mut ToolContext::new(), json!({ "summary": 3 }))
            .await
            .expect_err("schema");
        assert!(schema_error.message.contains("schema"));
        assert!(slot.take().is_none());
    }
}
