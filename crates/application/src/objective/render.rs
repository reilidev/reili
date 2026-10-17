//! What Reili writes on the tracker: the description of an item it opens and the progress comment
//! it leaves each cycle, in plain Markdown a teammate would write. Every text passes through
//! secret redaction and size limits; evidence appears as links, never as raw data.

use std::fmt::Write as _;

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    CycleReport, RecordRef, WorkItemProposal, WorkItemStatus, sanitize::sanitize_for_storage,
};

use super::policy::ScreenedProposal;

const MAX_TEXT_CHARS: usize = 2_000;
const MAX_LINE_CHARS: usize = 500;
const MAX_BODY_CHARS: usize = 30_000;

/// How a progress comment ends, depending on what Reili does with the item afterwards.
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
    pub screened: &'a ScreenedProposal,
    pub report: &'a CycleReport,
    pub follows_up: Option<&'a RecordRef>,
    pub run_id: &'a str,
}

#[must_use]
pub fn render_new_item_body(input: &RenderNewItemInput<'_>) -> String {
    let proposal = &input.screened.proposal;
    let mut body = text(&proposal.problem_statement, MAX_TEXT_CHARS);
    body.push('\n');
    if let Some(previous) = input.follows_up {
        let _ = writeln!(body, "\nFollows up on {}.", previous.key);
    }
    let _ = writeln!(body, "\n**Severity:** {}", proposal.severity.as_str());
    write_details(&mut body, proposal, input.report);
    write_notes(&mut body, &input.screened.notes);
    let _ = write!(
        body,
        "\n---\n_Opened by Reili for objective `{}` (run `{}`). Reili works on this while it is assigned to Reili and posts progress here; comment to steer it._\n",
        input.objective.id, input.run_id
    );
    text(&body, MAX_BODY_CHARS)
}

#[must_use]
pub fn render_progress_comment(
    screened: &ScreenedProposal,
    report: &CycleReport,
    run_id: &str,
    closing: ProgressClosing,
) -> String {
    let proposal = &screened.proposal;
    let mut body = format!(
        "**Progress** · `{}` · run `{run_id}`\n\n",
        proposal.status.as_str()
    );
    let log = text(&proposal.work_log, MAX_TEXT_CHARS);
    if !log.is_empty() {
        body.push_str(&log);
        body.push('\n');
    }
    write_details(&mut body, proposal, report);
    write_notes(&mut body, &screened.notes);
    match closing {
        ProgressClosing::Continue => {}
        ProgressClosing::Closing => {
            body.push_str("\nClosing as done. Reopen it if the problem comes back.\n");
        }
        ProgressClosing::SuggestClosing => {
            body.push_str("\nReili considers this done. Close it if you agree.\n");
        }
    }
    text(&body, MAX_BODY_CHARS)
}

fn write_details(body: &mut String, proposal: &WorkItemProposal, report: &CycleReport) {
    if proposal.status == WorkItemStatus::NeedsDecision
        && let Some(question) = &proposal.decision_needed
    {
        let _ = writeln!(
            body,
            "\n**Decision needed:** {}",
            text(question, MAX_LINE_CHARS)
        );
    }
    if !proposal.findings.is_empty() {
        body.push_str("\n**Findings**\n");
        for finding in &proposal.findings {
            let _ = writeln!(
                body,
                "- {}{}",
                text(&finding.statement, MAX_LINE_CHARS),
                citations(&finding.evidence_ids)
            );
        }
    }
    if !proposal.hypotheses.is_empty() {
        body.push_str("\n**Hypotheses**\n");
        for hypothesis in &proposal.hypotheses {
            let _ = writeln!(
                body,
                "- [{}] {}{}",
                hypothesis.status.as_str(),
                text(&hypothesis.statement, MAX_LINE_CHARS),
                citations(&hypothesis.evidence_ids)
            );
        }
    }
    if let Some(recommendation) = &proposal.recommendation {
        let _ = writeln!(
            body,
            "\n**Recommendation:** {}",
            text(&recommendation.action, MAX_LINE_CHARS)
        );
        for (label, value) in [
            ("Why", &recommendation.rationale),
            ("Expected effect", &recommendation.expected_effect),
            ("Risk", &recommendation.risk),
        ] {
            let value = text(value, MAX_LINE_CHARS);
            if !value.is_empty() {
                let _ = writeln!(body, "- {label}: {value}");
            }
        }
    }
    if !proposal.next_actions.is_empty() {
        body.push_str("\n**Next**\n");
        for action in &proposal.next_actions {
            let _ = writeln!(body, "- {}", text(action, MAX_LINE_CHARS));
        }
    }
    let evidence = cited_evidence(proposal, report);
    if !evidence.is_empty() {
        body.push_str("\n**Evidence**\n");
        for line in evidence {
            body.push_str(&line);
        }
    }
}

fn write_notes(body: &mut String, notes: &[String]) {
    for note in notes {
        let _ = writeln!(body, "\n_Note from Reili's report policy: {note}._");
    }
}

fn cited_evidence(proposal: &WorkItemProposal, report: &CycleReport) -> Vec<String> {
    let mut ids: Vec<&String> = proposal
        .evidence_ids
        .iter()
        .chain(proposal.findings.iter().flat_map(|f| f.evidence_ids.iter()))
        .chain(
            proposal
                .hypotheses
                .iter()
                .flat_map(|h| h.evidence_ids.iter()),
        )
        .collect();
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

fn citations(ids: &[String]) -> String {
    if ids.is_empty() {
        String::new()
    } else {
        format!(" ({})", ids.join(", "))
    }
}

fn text(value: &str, max: usize) -> String {
    sanitize_for_storage(value.trim(), max)
}

#[cfg(test)]
mod tests {
    use reili_core::work::{HypothesisStatus, WorkItemStatus};

    use super::{
        ProgressClosing, RenderNewItemInput, render_new_item_body, render_progress_comment,
    };
    use crate::objective::policy::ScreenedProposal;
    use crate::objective::policy::tests::{proposal, report};
    use crate::objective::tests_support::sample_objective;

    #[test]
    fn progress_comment_shows_the_work_question_and_evidence_links() {
        let mut proposal = proposal(HypothesisStatus::Supported);
        proposal.work_log =
            "Compared windows. Token ghp_abcdefghijklmnopqrstuvwxyz0123456789 leaked.".to_string();
        let screened = ScreenedProposal {
            proposal,
            notes: vec!["kept in progress".to_string()],
        };

        let comment = render_progress_comment(
            &screened,
            &report(Vec::new()),
            "run-1",
            ProgressClosing::Continue,
        );

        assert!(comment.contains("`needs-decision`"));
        assert!(comment.contains("**Decision needed:** Roll back deploy 42?"));
        assert!(comment.contains("[5xx dashboard](https://app.datadoghq.com/apm/checkout)"));
        assert!(comment.contains("report policy: kept in progress"));
        assert!(
            !comment.contains("ghp_abcdefghijklmnopqrstuvwxyz"),
            "secrets are redacted"
        );
    }

    #[test]
    fn new_item_body_states_the_problem_and_how_to_steer() {
        let mut proposal = proposal(HypothesisStatus::Proposed);
        proposal.status = WorkItemStatus::InProgress;
        let screened = ScreenedProposal {
            proposal,
            notes: Vec::new(),
        };

        let body = render_new_item_body(&RenderNewItemInput {
            objective: &sample_objective("checkout"),
            screened: &screened,
            report: &report(Vec::new()),
            follows_up: None,
            run_id: "run-1",
        });

        assert!(body.starts_with("5xx rate doubled"));
        assert!(body.contains("**Findings**"));
        assert!(body.contains("objective `checkout`"));
        assert!(!body.contains("Decision needed"));
    }
}
