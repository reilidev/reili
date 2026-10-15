//! Shared fixtures for tests in this crate and downstream crates (`test-support` feature).

use chrono::{TimeZone, Utc};

use super::item::{
    NotificationState, Severity, WORK_ITEM_SCHEMA_VERSION, WorkItemKey, WorkItemKind,
    WorkItemState, WorkItemStatus,
};
use crate::goal::GoalId;

#[must_use]
pub fn sample_work_item(key: &str) -> WorkItemState {
    let at = Utc
        .timestamp_opt(1_760_000_000, 0)
        .single()
        .unwrap_or_default();
    WorkItemState {
        schema_version: WORK_ITEM_SCHEMA_VERSION,
        key: WorkItemKey::normalize(key)
            .unwrap_or_else(|| WorkItemKey::normalize("sample").unwrap_or_else(|| unreachable!())),
        goal_id: GoalId::new("checkout").unwrap_or_else(|_| unreachable!()),
        title: format!("Sample {key}"),
        kind: WorkItemKind::Incident,
        severity: Severity::Medium,
        status: WorkItemStatus::Investigating,
        problem_statement: "sample problem".to_string(),
        plan: Vec::new(),
        hypotheses: Vec::new(),
        findings: Vec::new(),
        recommendation: None,
        evidence: Vec::new(),
        open_questions: Vec::new(),
        human_decision: None,
        next_actions: Vec::new(),
        outcome: None,
        feedback: Vec::new(),
        history: Vec::new(),
        last_run_id: None,
        created_at: at,
        updated_at: at,
        last_processed_comment_id: None,
        notification: NotificationState::default(),
        rerun_requested: false,
        revision: 0,
    }
}
