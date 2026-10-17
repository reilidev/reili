//! Report: compose a Slack message only for meaningful changes (new items, status changes,
//! decisions needed), with per-objective rate limits and an
//! idempotency key so a crash between "posted" and "recorded" does not double-post.

use std::fmt::Write as _;

use chrono::{DateTime, Duration, Utc};
use reili_core::objective::{ObjectiveDefinition, stable_fingerprint};
use reili_core::work::{
    ObjectiveLedgerState, Severity, StoredWorkItem, WorkItemStatus,
    sanitize::{neutralize_slack_broadcasts, redact_sensitive, truncate_chars},
};

const MAX_REPORT_CHARS: usize = 3_500;

/// Digest of everything a human would want to be told about again when it changes.
#[must_use]
pub fn notification_digest(item: &StoredWorkItem) -> String {
    let state = &item.state;
    stable_fingerprint(&format!(
        "{}|{}|{}|{}|{}",
        state.status.as_str(),
        state.severity.as_str(),
        state.title,
        state
            .human_decision
            .as_ref()
            .map_or("", |decision| decision.question.as_str()),
        state.findings.len(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportDecision {
    Nothing,
    Deferred {
        reason: String,
    },
    Post {
        text: String,
        item_numbers: Vec<u64>,
    },
}

pub struct ComposeReportInput<'a> {
    pub objective: &'a ObjectiveDefinition,
    pub ledger: &'a ObjectiveLedgerState,
    pub ledger_url: &'a str,
    pub items: &'a [StoredWorkItem],
    pub candidate_numbers: &'a [u64],
    pub summary: &'a str,
    pub run_id: &'a str,
    pub now: DateTime<Utc>,
}

#[must_use]
pub fn compose_report(input: &ComposeReportInput<'_>) -> ReportDecision {
    let changed: Vec<&StoredWorkItem> = input
        .items
        .iter()
        .filter(|item| input.candidate_numbers.contains(&item.issue.number))
        .filter(|item| {
            item.state.notification.last_digest.as_deref()
                != Some(notification_digest(item).as_str())
        })
        .collect();
    if changed.is_empty() {
        return ReportDecision::Nothing;
    }

    let today = input.now.date_naive();
    if input.ledger.reports_posted_on(today) >= input.objective.report.max_reports_per_day {
        return ReportDecision::Deferred {
            reason: format!(
                "daily report limit ({}) reached",
                input.objective.report.max_reports_per_day
            ),
        };
    }
    let urgent = changed.iter().any(|item| {
        item.state.severity == Severity::Critical || item.state.status == WorkItemStatus::NeedsHuman
    });
    if !urgent
        && let Some(last) = input.ledger.reports.last_report_at
        && input.now
            < last + Duration::minutes(i64::from(input.objective.report.min_interval_minutes))
    {
        return ReportDecision::Deferred {
            reason: format!(
                "minimum report interval ({} min) not elapsed",
                input.objective.report.min_interval_minutes
            ),
        };
    }

    ReportDecision::Post {
        text: render_report(input, &changed),
        item_numbers: changed.iter().map(|item| item.issue.number).collect(),
    }
}

fn render_report(input: &ComposeReportInput<'_>, changed: &[&StoredWorkItem]) -> String {
    let mut text = format!(
        "*Objective `{}`* — {}\n",
        input.objective.id,
        truncate_chars(input.summary.trim(), 600)
    );
    let decisions: Vec<&&StoredWorkItem> = changed
        .iter()
        .filter(|item| item.state.status == WorkItemStatus::NeedsHuman)
        .collect();
    if !decisions.is_empty() {
        text.push_str("\n*Needs your decision*\n");
        for item in decisions {
            let _ = writeln!(
                text,
                "• <{}|#{}> {} — {}",
                item.issue.url,
                item.issue.number,
                item.state.title,
                item.state
                    .human_decision
                    .as_ref()
                    .map_or("see issue", |decision| decision.question.as_str())
            );
        }
        text.push_str(
            "_Answer on the issue: comment, change its `reili-status:*` label, or close it._\n",
        );
    }
    let updates: Vec<&&StoredWorkItem> = changed
        .iter()
        .filter(|item| item.state.status != WorkItemStatus::NeedsHuman)
        .collect();
    if !updates.is_empty() {
        text.push_str("\n*Updates*\n");
        for item in updates {
            let _ = writeln!(
                text,
                "• <{}|#{}> [{} · {}] {} — {} finding(s)",
                item.issue.url,
                item.issue.number,
                item.state.status.as_str(),
                item.state.severity.as_str(),
                item.state.title,
                item.state.findings.len(),
            );
        }
    }
    let _ = write!(
        text,
        "\nHypotheses are not conclusions. Details: <{}|objective ledger> · run `{}`",
        input.ledger_url, input.run_id
    );
    truncate_chars(
        &neutralize_slack_broadcasts(&redact_sensitive(&text)),
        MAX_REPORT_CHARS,
    )
}

#[must_use]
pub fn report_key(objective_id: &str, run_id: &str) -> String {
    format!("{objective_id}:{run_id}")
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use reili_core::work::{
        HumanDecisionRequest, IssueRef, ObjectiveLedgerState, StoredWorkItem, WorkItemStatus,
        test_support::sample_work_item,
    };

    use super::{ComposeReportInput, ReportDecision, compose_report, notification_digest};
    use crate::objective::testing::at;
    use crate::objective::tests_support::sample_objective;

    fn stored(number: u64, status: WorkItemStatus) -> StoredWorkItem {
        let mut state = sample_work_item(&format!("checkout:{number}"));
        state.status = status;
        if status == WorkItemStatus::NeedsHuman {
            state.human_decision = Some(HumanDecisionRequest {
                question: "Roll back <!channel>?".to_string(),
                requested_at: at("2026-10-15T00:00:00Z"),
            });
        }
        StoredWorkItem {
            issue: IssueRef {
                number,
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
            state,
            human_notes: String::new(),
            observed_status: status,
            remote_updated_at: at("2026-10-15T00:00:00Z"),
        }
    }

    #[test]
    fn posts_changed_items_and_skips_already_notified() {
        let objective = sample_objective("checkout");
        let ledger = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        let mut notified = stored(2, WorkItemStatus::Investigating);
        notified.state.notification.last_digest = Some(notification_digest(&notified));
        let items = vec![stored(1, WorkItemStatus::NeedsHuman), notified];

        let decision = compose_report(&ComposeReportInput {
            objective: &objective,
            ledger: &ledger,
            ledger_url: "https://github.com/acme/work/issues/9",
            items: &items,
            candidate_numbers: &[1, 2],
            summary: "Found a regression.",
            run_id: "run-1",
            now: at("2026-10-15T01:00:00Z"),
        });

        let ReportDecision::Post { text, item_numbers } = decision else {
            panic!("expected post");
        };
        assert_eq!(item_numbers, vec![1]);
        assert!(text.contains("Needs your decision"));
        assert!(
            !text.contains("<!channel>"),
            "broadcast mentions are neutralized"
        );
        assert!(text.contains("Hypotheses are not conclusions"));
    }

    #[test]
    fn stays_silent_without_changes() {
        let objective = sample_objective("checkout");
        let ledger = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));

        let decision = compose_report(&ComposeReportInput {
            objective: &objective,
            ledger: &ledger,
            ledger_url: "",
            items: &[],
            candidate_numbers: &[],
            summary: "No new findings.",
            run_id: "run-1",
            now: at("2026-10-15T01:00:00Z"),
        });

        assert_eq!(decision, ReportDecision::Nothing);
    }

    #[test]
    fn rate_limits_non_urgent_reports() {
        let objective = sample_objective("checkout");
        let mut ledger = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        ledger.remember_report_key("k".to_string(), at("2026-10-15T00:30:00Z"));
        let items = vec![stored(1, WorkItemStatus::Investigating)];

        let decision = compose_report(&ComposeReportInput {
            objective: &objective,
            ledger: &ledger,
            ledger_url: "",
            items: &items,
            candidate_numbers: &[1],
            summary: "s",
            run_id: "run-1",
            now: at("2026-10-15T00:30:00Z") + Duration::minutes(10),
        });

        assert!(matches!(decision, ReportDecision::Deferred { .. }));
    }
}
