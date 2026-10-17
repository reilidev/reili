//! Report: what a cycle did on the tracker, and the Slack message composed from it. Reili posts
//! only when there is something the team should look at (new items, decisions needed, items done);
//! routine progress stays on the items.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    RecordRef, Severity,
    sanitize::{neutralize_slack_broadcasts, redact_sensitive, truncate_chars},
};

const MAX_REPORT_CHARS: usize = 3_500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLine {
    pub record: RecordRef,
    pub title: String,
    pub severity: Severity,
    pub question: Option<String>,
}

/// What one cycle did on the tracker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub created: Vec<ItemLine>,
    pub updated: Vec<ItemLine>,
    /// Items that started waiting for a team decision in this cycle.
    pub needs_decision: Vec<ItemLine>,
    pub resolved: Vec<ItemLine>,
    pub skipped: Vec<String>,
}

impl Activity {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty()
    }
}

/// Key of the Slack report for one scheduled slot, so a re-run of the slot never posts twice.
#[must_use]
pub fn report_key(objective: &ObjectiveDefinition, slot: DateTime<Utc>) -> String {
    format!("{}:{}", objective.id, slot.to_rfc3339())
}

/// `None` when nothing in the cycle needs the team's attention.
#[must_use]
pub fn compose_report(
    objective: &ObjectiveDefinition,
    summary: &str,
    run_id: &str,
    activity: &Activity,
) -> Option<String> {
    if activity.created.is_empty()
        && activity.needs_decision.is_empty()
        && activity.resolved.is_empty()
    {
        return None;
    }
    let mut text = format!(
        "*Objective `{}`* — {}\n",
        objective.id,
        truncate_chars(summary.trim(), 600)
    );
    if !activity.needs_decision.is_empty() {
        text.push_str("\n*Needs your decision*\n");
        for item in &activity.needs_decision {
            let _ = writeln!(
                text,
                "• <{}|{}> {} — {}",
                item.record.url,
                item.record.key,
                item.title,
                item.question.as_deref().unwrap_or("see the item")
            );
        }
        text.push_str("_Answer with a comment on the item._\n");
    }
    write_section(&mut text, "New work items", &activity.created);
    write_section(&mut text, "Done", &activity.resolved);
    let _ = write!(text, "\nHypotheses are not conclusions. Run `{run_id}`");
    Some(truncate_chars(
        &neutralize_slack_broadcasts(&redact_sensitive(&text)),
        MAX_REPORT_CHARS,
    ))
}

fn write_section(text: &mut String, heading: &str, items: &[ItemLine]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(text, "\n*{heading}*");
    for item in items {
        let _ = writeln!(
            text,
            "• <{}|{}> [{}] {}",
            item.record.url,
            item.record.key,
            item.severity.as_str(),
            item.title
        );
    }
}

#[cfg(test)]
mod tests {
    use reili_core::work::{RecordKey, RecordRef, Severity};

    use super::{Activity, ItemLine, compose_report};
    use crate::objective::tests_support::sample_objective;

    fn line(number: u64, question: Option<&str>) -> ItemLine {
        ItemLine {
            record: RecordRef {
                key: RecordKey::new(format!("#{number}")),
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
            title: format!("Item {number}"),
            severity: Severity::High,
            question: question.map(str::to_string),
        }
    }

    #[test]
    fn reports_decisions_and_new_items() {
        let activity = Activity {
            created: vec![line(1, None)],
            updated: vec![line(2, Some("Roll back <!channel>?"))],
            needs_decision: vec![line(2, Some("Roll back <!channel>?"))],
            ..Activity::default()
        };

        let text = compose_report(
            &sample_objective("checkout"),
            "Found a regression.",
            "run-1",
            &activity,
        )
        .expect("report");

        assert!(text.contains("Needs your decision"));
        assert!(text.contains("<https://github.com/acme/work/issues/1|#1>"));
        assert!(
            !text.contains("<!channel>"),
            "broadcast mentions are neutralized"
        );
    }

    #[test]
    fn stays_quiet_on_routine_progress() {
        let activity = Activity {
            updated: vec![line(2, None)],
            ..Activity::default()
        };

        assert_eq!(
            compose_report(&sample_objective("checkout"), "s", "run-1", &activity),
            None
        );
    }
}
