use std::collections::BTreeSet;

use reili_core::cycle_agent::{ReportSlot, SUBMIT_CYCLE_REPORT_TOOL_NAME};
use reili_core::error::PortError;
use reili_core::work::{CycleReport, WorkItemStatus};
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
            if item.title.trim().is_empty() {
                problems.push(format!("work item `{}` needs a title", item.key));
            }
            let continues_existing = item
                .existing_record
                .as_deref()
                .is_some_and(|record| !record.trim().is_empty());
            if continues_existing && item.comment.trim().is_empty() {
                problems.push(format!(
                    "work item `{}` continues an existing item and needs a `comment`",
                    item.key
                ));
            }
            if !continues_existing && item.description.trim().is_empty() {
                problems.push(format!(
                    "work item `{}` is new and needs a `description`",
                    item.key
                ));
            }
            if item.status == WorkItemStatus::Resolved && item.evidence_ids.is_empty() {
                problems.push(format!(
                    "work item `{}` is `resolved` and needs at least one id in `evidence_ids`",
                    item.key
                ));
            }
            if item.status == WorkItemStatus::NeedsDecision
                && item
                    .decision_needed
                    .as_deref()
                    .is_none_or(|question| question.trim().is_empty())
            {
                problems.push(format!(
                    "work item `{}` is `needs_decision` and needs a concrete question in `decision_needed`",
                    item.key
                ));
            }
            for id in &item.evidence_ids {
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
    const NAME: &'static str = SUBMIT_CYCLE_REPORT_TOOL_NAME;

    type Error = PortError;
    type Args = Value;
    type Output = String;

    fn description(&self) -> String {
        "Submit the final structured report for this objective cycle. Call exactly once, at the end. Reili validates it and writes it to the team's tracker: a progress comment on each of your items you worked on, and a new item (assigned to you) for new work. Cited evidence ids must exist in `evidence`.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "slack_message": { "type": "string", "description": "Optional. Your message to the team's report channel in Slack mrkdwn, written for what happened this cycle. Omit it when nothing needs the team's attention. Write [[key]] to refer to a work item in this report; Reili turns it into a link to the item." },
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
            "required": ["no_new_findings"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let report: CycleReport = serde_json::from_value(args).map_err(|error| {
            PortError::invalid_input(format!(
                "report does not match the schema: {error}. Fix it and call {SUBMIT_CYCLE_REPORT_TOOL_NAME} again."
            ))
        })?;
        let problems = self.validate(&report);
        if !problems.is_empty() {
            return Err(PortError::invalid_input(format!(
                "report rejected: {}. Fix these and call {SUBMIT_CYCLE_REPORT_TOOL_NAME} again.",
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

fn evidence_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "E1, E2, ..." },
            "url": { "type": "string" },
            "description": { "type": "string" },
            "query": { "type": "string" }
        },
        "required": ["id", "description"]
    })
}

fn work_item_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Short name for this item within the report, such as `db-pool-timeouts`. Write [[key]] in slack_message to link to it." },
            "existing_record": { "type": "string", "description": "Record key of the item this is about, exactly as shown in the brief (e.g. `#12`). Without it, a new item is opened." },
            "title": { "type": "string" },
            "status": { "type": "string", "enum": ["in_progress", "needs_decision", "resolved"], "description": "needs_decision asks the team a question (decision_needed); resolved needs at least one cited evidence id." },
            "description": { "type": "string", "description": "For new work: the item's description in GitHub Markdown, written in the shape the work calls for. Required for new work." },
            "comment": { "type": "string", "description": "When continuing an existing item: your progress comment in GitHub Markdown, written in the shape the work calls for. Required when continuing an existing item." },
            "decision_needed": { "type": "string", "description": "The concrete question for the team when status is needs_decision. Reili appends it to the text." },
            "evidence_ids": { "type": "array", "items": { "type": "string" }, "description": "Evidence the text relies on. Reili links each one at the end of the text." }
        },
        "required": ["key", "title", "status"]
    })
}

fn knowledge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "fact": { "type": "string" },
            "evidence": { "type": "string" },
            "scope": { "type": "string" },
            "shared": { "type": "boolean" }
        },
        "required": ["fact", "evidence", "scope"]
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
            "status": "in_progress",
            "description": "p",
            "evidence_ids": [evidence]
        })
    }

    fn tool() -> (SubmitCycleReportTool, ReportSlot) {
        let slot = ReportSlot::new();
        (SubmitCycleReportTool::new(slot.clone()), slot)
    }

    #[tokio::test]
    async fn accepts_valid_report() {
        let (tool, slot) = tool();

        tool.call(
            &mut ToolContext::new(),
            json!({
                                "no_new_findings": false,
                "evidence": [{ "id": "E1", "url": "https://x", "description": "d" }],
                "work_items": [item("E1")]
            }),
        )
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
                                        "no_new_findings": false,
                    "evidence": [],
                    "work_items": [item("E9")]
                }),
            )
            .await
            .expect_err("rejected");
        assert!(error.message.contains("evidence `E9`"));

        let mut continuation = item("E9");
        continuation["existing_record"] = json!("#3");
        let missing_comment = tool
            .call(
                &mut ToolContext::new(),
                json!({
                                        "no_new_findings": false,
                    "evidence": [{ "id": "E9", "description": "d" }],
                    "work_items": [continuation]
                }),
            )
            .await
            .expect_err("rejected");
        assert!(missing_comment.message.contains("`comment`"));

        let mut unproven = item("E9");
        unproven["status"] = json!("resolved");
        unproven["evidence_ids"] = json!([]);
        let mut unasked = item("E9");
        unasked["key"] = json!("checkout:latency");
        unasked["status"] = json!("needs_decision");
        let status_errors = tool
            .call(
                &mut ToolContext::new(),
                json!({
                                        "no_new_findings": false,
                    "evidence": [{ "id": "E9", "description": "d" }],
                    "work_items": [unproven, unasked]
                }),
            )
            .await
            .expect_err("rejected");
        assert!(status_errors.message.contains("is `resolved` and needs"));
        assert!(
            status_errors
                .message
                .contains("is `needs_decision` and needs")
        );

        let schema_error = tool
            .call(&mut ToolContext::new(), json!({ "slack_message": 3 }))
            .await
            .expect_err("schema");
        assert!(schema_error.message.contains("schema"));
        assert!(slot.take().is_none());
    }
}
