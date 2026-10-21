use std::fmt::Write as _;

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    CycleReport, RecordRef, WorkItemProposal, WorkItemStatus, text::truncate_chars,
};

const MAX_MARKDOWN_CHARS: usize = 20_000;
const MAX_LINE_CHARS: usize = 500;
const MAX_BODY_CHARS: usize = 30_000;
const NEXT_STEP_LABEL: &str = "**Next step:**";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressClosing {
    Continue,
    /// Reili closes its own item as done.
    Closing,
    /// The item belongs to the team; Reili suggests closing it.
    SuggestClosing,
}

pub struct RenderNewItemInput<'a> {
    pub objective: &'a ObjectiveDefinition,
    pub proposal: &'a WorkItemProposal,
    pub report: &'a CycleReport,
    pub follows_up: Option<&'a RecordRef>,
    pub run_id: &'a str,
}

#[must_use]
pub fn render_new_item_body(input: &RenderNewItemInput<'_>) -> String {
    let proposal = input.proposal;
    let mut body = text(&proposal.description, MAX_MARKDOWN_CHARS);
    body.push('\n');
    if let Some(previous) = input.follows_up {
        let _ = writeln!(body, "\nFollows up on {}.", previous.key);
    }
    write_appendix(&mut body, proposal, input.report);
    let _ = write!(
        body,
        "\n---\n_Opened by Reili for objective `{}` (run `{}`). Reili works on this while it is assigned to Reili and posts progress here; comment to steer it._\n",
        input.objective.id, input.run_id
    );
    text(&body, MAX_BODY_CHARS)
}

#[must_use]
pub fn render_progress_comment(
    proposal: &WorkItemProposal,
    report: &CycleReport,
    run_id: &str,
    closing: ProgressClosing,
) -> String {
    let mut body = text(&proposal.comment, MAX_MARKDOWN_CHARS);
    body.push('\n');
    write_appendix(&mut body, proposal, report);
    match closing {
        ProgressClosing::Continue => {}
        ProgressClosing::Closing => {
            body.push_str("\nClosing as done. Reopen it if the problem comes back.\n");
        }
        ProgressClosing::SuggestClosing => {
            body.push_str("\nReili considers this done. Close it if you agree.\n");
        }
    }
    let _ = write!(
        body,
        "\n---\n_Status `{}` · run `{run_id}`_\n",
        proposal.status.as_str()
    );
    text(&body, MAX_BODY_CHARS)
}

/// The next step Reili recorded in a description or comment it rendered.
#[must_use]
pub fn recorded_next_step(body: &str) -> Option<&str> {
    body.lines()
        .rev()
        .find_map(|line| line.strip_prefix(NEXT_STEP_LABEL))
        .map(str::trim)
        .filter(|step| !step.is_empty())
}

fn write_appendix(body: &mut String, proposal: &WorkItemProposal, report: &CycleReport) {
    if proposal.status == WorkItemStatus::NeedsDecision
        && let Some(question) = &proposal.decision_needed
    {
        let _ = writeln!(
            body,
            "\n**Decision needed:** {}",
            text(question, MAX_LINE_CHARS)
        );
    }
    if proposal.status != WorkItemStatus::Resolved
        && let Some(step) = proposal.next_step.as_deref().map(single_line)
        && !step.is_empty()
    {
        let _ = writeln!(body, "\n{NEXT_STEP_LABEL} {step}");
    }
    let evidence = cited_evidence(proposal, report);
    if !evidence.is_empty() {
        body.push_str("\n**Evidence**\n");
        for line in evidence {
            body.push_str(&line);
        }
    }
}

fn cited_evidence(proposal: &WorkItemProposal, report: &CycleReport) -> Vec<String> {
    let mut ids: Vec<&String> = proposal.evidence_ids.iter().collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| report.evidence_by_id(id))
        .map(|evidence| {
            let description = text(&evidence.description, MAX_LINE_CHARS);
            let source = evidence.url.as_deref().map_or_else(
                || description.clone(),
                |url| format!("[{description}]({})", text(url, MAX_LINE_CHARS)),
            );
            let query = evidence
                .query
                .as_deref()
                .map(|query| format!(" — query `{}`", text(query, MAX_LINE_CHARS)))
                .unwrap_or_default();
            format!("- {}: {source}{query}\n", evidence.id)
        })
        .collect()
}

fn text(value: &str, max: usize) -> String {
    truncate_chars(value.trim(), max)
}

/// Keeps the line [`recorded_next_step`] reads back on one line.
fn single_line(value: &str) -> String {
    text(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_LINE_CHARS,
    )
}

#[cfg(test)]
mod tests {
    use reili_core::work::WorkItemStatus;

    use super::{
        ProgressClosing, RenderNewItemInput, recorded_next_step, render_new_item_body,
        render_progress_comment,
    };
    use crate::objective::tests_support::{proposal, report, sample_objective};

    #[test]
    fn progress_comment_keeps_reilis_markdown_and_appends_the_question_and_evidence() {
        let mut proposal = proposal();
        proposal.comment =
            "## Review of the retry settings\n\n| Service | Retries |\n|---|---|\n| checkout | 3 |"
                .to_string();

        let comment = render_progress_comment(
            &proposal,
            &report(Vec::new()),
            "run-1",
            ProgressClosing::Continue,
        );

        assert!(comment.starts_with("## Review of the retry settings\n\n| Service | Retries |"));
        assert!(!comment.contains("**Findings**"));
        assert!(comment.contains("_Status `needs-decision` · run `run-1`_"));
        assert!(comment.contains("**Decision needed:** Roll back deploy 42?"));
        assert!(comment.contains("[5xx dashboard](https://app.datadoghq.com/apm/checkout)"));
    }

    #[test]
    fn new_item_body_states_the_problem_and_how_to_steer() {
        let mut proposal = proposal();
        proposal.status = WorkItemStatus::InProgress;

        let body = render_new_item_body(&RenderNewItemInput {
            objective: &sample_objective("checkout"),
            proposal: &proposal,
            report: &report(Vec::new()),
            follows_up: None,
            run_id: "run-1",
        });

        assert!(body.starts_with("5xx rate doubled"));
        assert!(body.contains("**Evidence**"));
        assert!(body.contains("objective `checkout`"));
        assert!(!body.contains("Decision needed"));
    }

    #[test]
    fn records_the_next_step_where_the_next_cycle_reads_it_back() {
        let mut proposal = proposal();
        proposal.status = WorkItemStatus::InProgress;
        proposal.comment = "Checked the pool.\n\n**Next step:** not this one".to_string();
        proposal.next_step = Some("Compare pool waits\nbefore and after deploy 43".to_string());

        let comment = render_progress_comment(
            &proposal,
            &report(Vec::new()),
            "run-1",
            ProgressClosing::Continue,
        );

        assert!(
            comment.contains("\n**Next step:** Compare pool waits before and after deploy 43\n")
        );
        assert_eq!(
            recorded_next_step(&comment),
            Some("Compare pool waits before and after deploy 43")
        );
    }

    #[test]
    fn records_no_next_step_for_resolved_items() {
        let mut proposal = proposal();
        proposal.status = WorkItemStatus::Resolved;
        proposal.next_step = Some("Keep watching".to_string());

        let comment = render_progress_comment(
            &proposal,
            &report(Vec::new()),
            "run-1",
            ProgressClosing::Closing,
        );

        assert_eq!(recorded_next_step(&comment), None);
    }
}
