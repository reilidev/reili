//! Report: compose a Slack message only for meaningful changes (new items, status changes,
//! decisions needed, outcome verdicts, criteria breaches), with per-goal rate limits and an
//! idempotency key so a crash between "posted" and "recorded" does not double-post.

use std::fmt::Write as _;

use chrono::{DateTime, Duration, Utc};
use reili_core::goal::{GoalDefinition, stable_fingerprint};
use reili_core::work::{
    CriterionObservation, GoalLedgerState, Severity, StoredWorkItem, WorkItemStatus,
    sanitize::{neutralize_slack_broadcasts, redact_sensitive, truncate_chars},
};

const MAX_REPORT_CHARS: usize = 3_500;

/// Digest of everything a human would want to be told about again when it changes.
#[must_use]
pub fn notification_digest(item: &StoredWorkItem) -> String {
    let state = &item.state;
    stable_fingerprint(&format!(
        "{}|{}|{}|{}|{}|{}",
        state.status.as_str(),
        state.severity.as_str(),
        state.title,
        state
            .human_decision
            .as_ref()
            .map_or("", |decision| decision.question.as_str()),
        state
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.latest_verdict())
            .map_or("", |verdict| verdict.as_str()),
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
    pub goal: &'a GoalDefinition,
    pub ledger: &'a GoalLedgerState,
    pub ledger_url: &'a str,
    pub items: &'a [StoredWorkItem],
    pub candidate_numbers: &'a [u64],
    pub criteria: &'a [(String, CriterionObservation)],
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
    let breaches = newly_breached_criteria(input);
    if changed.is_empty() && breaches.is_empty() {
        return ReportDecision::Nothing;
    }

    let today = input.now.date_naive();
    if input.ledger.reports_posted_on(today) >= input.goal.report.max_reports_per_day {
        return ReportDecision::Deferred {
            reason: format!(
                "daily report limit ({}) reached",
                input.goal.report.max_reports_per_day
            ),
        };
    }
    let urgent = changed.iter().any(|item| {
        item.state.severity == Severity::Critical || item.state.status == WorkItemStatus::NeedsHuman
    });
    if !urgent
        && let Some(last) = input.ledger.reports.last_report_at
        && input.now < last + Duration::minutes(i64::from(input.goal.report.min_interval_minutes))
    {
        return ReportDecision::Deferred {
            reason: format!(
                "minimum report interval ({} min) not elapsed",
                input.goal.report.min_interval_minutes
            ),
        };
    }

    ReportDecision::Post {
        text: render_report(input, &changed, &breaches),
        item_numbers: changed.iter().map(|item| item.issue.number).collect(),
    }
}

fn newly_breached_criteria<'a>(
    input: &'a ComposeReportInput<'_>,
) -> Vec<&'a (String, CriterionObservation)> {
    input
        .criteria
        .iter()
        .filter(|(id, observation)| {
            if observation.met != Some(false) {
                return false;
            }
            let previous = input
                .ledger
                .criteria
                .iter()
                .find(|history| &history.criterion_id == id)
                .and_then(|history| {
                    history
                        .observations
                        .iter()
                        .rev()
                        .find(|previous| previous.at < observation.at)
                })
                .and_then(|previous| previous.met);
            previous != Some(false)
        })
        .collect()
}

fn render_report(
    input: &ComposeReportInput<'_>,
    changed: &[&StoredWorkItem],
    breaches: &[&(String, CriterionObservation)],
) -> String {
    let mut text = format!(
        "*Goal `{}`* — {}\n",
        input.goal.id,
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
        text.push_str("_Reply on the issue with `/reili approve`, `/reili dismiss <reason>`, or `/reili implemented <what changed>`._\n");
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
                "• <{}|#{}> [{} · {}] {} — {} finding(s){}",
                item.issue.url,
                item.issue.number,
                item.state.status.as_str(),
                item.state.severity.as_str(),
                item.state.title,
                item.state.findings.len(),
                item.state
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.latest_verdict())
                    .map(|verdict| format!("; outcome: {}", verdict.as_str()))
                    .unwrap_or_default()
            );
        }
    }
    if !breaches.is_empty() {
        text.push_str("\n*Success criteria not met*\n");
        for (id, observation) in breaches {
            let _ = writeln!(
                text,
                "• {id}: {}{}",
                observation.detail,
                observation
                    .source_url
                    .as_deref()
                    .map(|url| format!(" (<{url}|data>)"))
                    .unwrap_or_default()
            );
        }
    }
    let _ = write!(
        text,
        "\nHypotheses are not conclusions. Details: <{}|goal ledger> · run `{}`",
        input.ledger_url, input.run_id
    );
    truncate_chars(
        &neutralize_slack_broadcasts(&redact_sensitive(&text)),
        MAX_REPORT_CHARS,
    )
}

#[must_use]
pub fn report_key(goal_id: &str, run_id: &str) -> String {
    format!("{goal_id}:{run_id}")
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use reili_core::work::{
        CriterionObservation, GoalLedgerState, HumanDecisionRequest, IssueRef, StoredWorkItem,
        WorkItemStatus, test_support::sample_work_item,
    };

    use super::{ComposeReportInput, ReportDecision, compose_report, notification_digest};
    use crate::goal::testing::at;
    use crate::goal::tests_support::sample_goal;

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
        let goal = sample_goal("checkout");
        let ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        let mut notified = stored(2, WorkItemStatus::Investigating);
        notified.state.notification.last_digest = Some(notification_digest(&notified));
        let items = vec![stored(1, WorkItemStatus::NeedsHuman), notified];

        let decision = compose_report(&ComposeReportInput {
            goal: &goal,
            ledger: &ledger,
            ledger_url: "https://github.com/acme/work/issues/9",
            items: &items,
            candidate_numbers: &[1, 2],
            criteria: &[],
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
        let goal = sample_goal("checkout");
        let ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));

        let decision = compose_report(&ComposeReportInput {
            goal: &goal,
            ledger: &ledger,
            ledger_url: "",
            items: &[],
            candidate_numbers: &[],
            criteria: &[],
            summary: "No new findings.",
            run_id: "run-1",
            now: at("2026-10-15T01:00:00Z"),
        });

        assert_eq!(decision, ReportDecision::Nothing);
    }

    #[test]
    fn rate_limits_non_urgent_reports() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        ledger.remember_report_key("k".to_string(), at("2026-10-15T00:30:00Z"));
        let items = vec![stored(1, WorkItemStatus::Investigating)];

        let decision = compose_report(&ComposeReportInput {
            goal: &goal,
            ledger: &ledger,
            ledger_url: "",
            items: &items,
            candidate_numbers: &[1],
            criteria: &[],
            summary: "s",
            run_id: "run-1",
            now: at("2026-10-15T00:30:00Z") + Duration::minutes(10),
        });

        assert!(matches!(decision, ReportDecision::Deferred { .. }));
    }

    #[test]
    fn reports_newly_breached_criteria_once() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        let breach = CriterionObservation {
            at: at("2026-10-15T06:00:00Z"),
            value: Some(0.05),
            met: Some(false),
            detail: "0.05 <= 0.01 (not met)".to_string(),
            source_url: None,
        };
        let input_criteria = vec![("error-rate".to_string(), breach.clone())];

        let first = compose_report(&ComposeReportInput {
            goal: &goal,
            ledger: &ledger,
            ledger_url: "",
            items: &[],
            candidate_numbers: &[],
            criteria: &input_criteria,
            summary: "s",
            run_id: "run-1",
            now: at("2026-10-15T06:00:00Z"),
        });
        assert!(matches!(first, ReportDecision::Post { .. }));

        let mut earlier = breach.clone();
        earlier.at = at("2026-10-15T05:00:00Z");
        ledger.record_criterion("error-rate", earlier);
        let second = compose_report(&ComposeReportInput {
            goal: &goal,
            ledger: &ledger,
            ledger_url: "",
            items: &[],
            candidate_numbers: &[],
            criteria: &input_criteria,
            summary: "s",
            run_id: "run-2",
            now: at("2026-10-15T06:00:00Z"),
        });
        assert_eq!(second, ReportDecision::Nothing);
    }
}
