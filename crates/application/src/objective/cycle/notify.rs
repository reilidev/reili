//! Report: post meaningful changes to Slack once (idempotency key), mark the reported items, and
//! post one-off notices for cycles that ended without results.

use reili_core::error::PortError;
use reili_core::messaging::slack::ObjectiveReportMessage;
use reili_core::objective::{ObjectiveDefinition, stable_fingerprint};
use reili_core::work::{ActiveCycle, CyclePhase, CycleResult, StoredLedger, WorkStateError};

use super::super::report::{
    ComposeReportInput, ReportDecision, compose_report, notification_digest, report_key,
};
use super::ObjectiveCycleRunner;

pub(super) enum ReportError {
    WorkState(WorkStateError),
    Slack(PortError),
}

impl From<WorkStateError> for ReportError {
    fn from(error: WorkStateError) -> Self {
        Self::WorkState(error)
    }
}

impl ObjectiveCycleRunner {
    pub(super) async fn report(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
    ) -> Result<StoredLedger, ReportError> {
        let now = self.deps.clock.now();
        let items = self
            .deps
            .store
            .repository()
            .list_work_items(&objective.id)
            .await?;
        let mut candidates = active.pending_notifications.clone();
        candidates.extend(
            items
                .iter()
                .filter(|item| item.state.status.is_open())
                .filter(|item| {
                    item.state.notification.last_digest.as_deref()
                        != Some(notification_digest(item).as_str())
                })
                .map(|item| item.record.key.clone()),
        );
        let decision = compose_report(&ComposeReportInput {
            objective,
            ledger: &ledger.state,
            ledger_url: &ledger.record.url,
            items: &items,
            candidate_keys: &candidates,
            summary: &active.summary,
            run_id: &active.run_id,
            now,
        });

        let mut posted_key = None;
        match decision {
            ReportDecision::Post { text, item_keys } if objective.permissions.post_reports => {
                let key = report_key(objective.id.as_str(), &active.run_id);
                let already = self
                    .deps
                    .report_port
                    .find_posted_report(&objective.report.channel_id, &key)
                    .await
                    .map_err(ReportError::Slack)?;
                if already.is_none() {
                    self.deps
                        .report_port
                        .post_report(ObjectiveReportMessage {
                            channel_id: objective.report.channel_id.clone(),
                            objective_id: objective.id.to_string(),
                            report_key: key.clone(),
                            text,
                        })
                        .await
                        .map_err(ReportError::Slack)?;
                }
                for item in items
                    .iter()
                    .filter(|item| item_keys.contains(&item.record.key))
                {
                    let digest = notification_digest(item);
                    self.deps
                        .store
                        .mutate_work_item(item, |state| {
                            state.notification.last_digest = Some(digest.clone());
                            state.notification.last_notified_at = Some(now);
                        })
                        .await?;
                }
                posted_key = Some(key);
            }
            ReportDecision::Post { .. } | ReportDecision::Nothing => {}
            ReportDecision::Deferred { reason } => {
                self.log(
                    "objective_cycle_report_deferred",
                    objective,
                    active,
                    Some(&reason),
                );
            }
        }

        let (saved, ()) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                if let Some(key) = &posted_key
                    && !state.reports.recent_report_keys.contains(key)
                {
                    state.remember_report_key(key.clone(), now);
                    state.usage_for(now.date_naive()).reports_posted += 1;
                }
                if let Some(cycle) = state.active_cycle.as_mut() {
                    cycle.phase = CyclePhase::Learning;
                }
                state.updated_at = now;
            })
            .await?;
        Ok(saved)
    }

    /// Posts a one-off notice for cycles that ended without results humans should know about.
    pub(super) async fn notify_terminal_result(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        result: &CycleResult,
    ) {
        let message = match result {
            CycleResult::Abandoned { reason } => format!(
                "*Objective `{}`*: a cycle was abandoned — {reason}. Reili will try again at the next scheduled time. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            CycleResult::BudgetExhausted { reason } => format!(
                "*Objective `{}`*: a cycle stopped on its budget — {reason}. Partial results were not recorded as findings. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            CycleResult::Skipped { reason } if reason.contains("budget") => format!(
                "*Objective `{}`*: skipped a cycle — {reason}. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            _ => return,
        };
        if !objective.permissions.post_reports {
            return;
        }
        let today = self.deps.clock.now().date_naive();
        let key = format!(
            "{}:notice:{}:{}",
            objective.id,
            today,
            stable_fingerprint(&message)
        );
        if ledger.state.reports.recent_report_keys.contains(&key) {
            return;
        }
        let posted = self
            .deps
            .report_port
            .find_posted_report(&objective.report.channel_id, &key)
            .await;
        if !matches!(posted, Ok(None)) {
            return;
        }
        let post = self
            .deps
            .report_port
            .post_report(ObjectiveReportMessage {
                channel_id: objective.report.channel_id.clone(),
                objective_id: objective.id.to_string(),
                report_key: key.clone(),
                text: message,
            })
            .await;
        if post.is_ok() {
            let now = self.deps.clock.now();
            let _ = self
                .deps
                .store
                .mutate_ledger(ledger, |state| state.remember_report_key(key.clone(), now))
                .await;
        }
    }
}
