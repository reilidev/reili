//! Decide: write the screened report to the tracker as a teammate would — a progress comment on
//! each of Reili's items it worked on, the needs-decision label when it waits for the team, closing
//! its own items when done, and new items (labelled and assigned to Reili) for new work.

use chrono::Duration;
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    CloseReason, CycleReport, NewWorkItem, RecordRef, WorkItem, WorkItemKey, WorkItemState,
    WorkItemStatus, WorkStateError, sanitize::sanitize_for_storage,
};

use super::super::decide::{PlanChangesInput, PlannedChange, plan_changes};
use super::super::policy::ScreenedProposal;
use super::super::render::{
    ProgressClosing, RenderNewItemInput, render_new_item_body, render_progress_comment,
};
use super::super::report::{Activity, ItemLine};
use super::ObjectiveCycleRunner;
use super::observe::Observed;

const MAX_TITLE_CHARS: usize = 200;
const DUPLICATE_LOOKBACK_DAYS: i64 = 1;

impl ObjectiveCycleRunner {
    pub(super) async fn apply(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        observed: &Observed,
        report: &CycleReport,
        proposals: &[ScreenedProposal],
    ) -> Result<Activity, WorkStateError> {
        let mut activity = Activity::default();
        let changes = plan_changes(&PlanChangesInput {
            objective,
            items: &observed.items,
            proposals,
        });
        for change in changes {
            match change {
                PlannedChange::Update { item, proposal } => {
                    self.continue_item(&item, &proposal, report, run_id, &mut activity)
                        .await?;
                }
                PlannedChange::Create {
                    key,
                    proposal,
                    follows_up,
                } => {
                    let body = render_new_item_body(&RenderNewItemInput {
                        objective,
                        screened: &proposal,
                        report,
                        follows_up: follows_up.as_ref(),
                        run_id,
                    });
                    self.open_item(
                        objective,
                        key,
                        &proposal,
                        body,
                        report,
                        run_id,
                        &mut activity,
                    )
                    .await?;
                }
                PlannedChange::Skipped { title, reason } => {
                    activity.skipped.push(format!("{title}: {reason}"));
                }
            }
        }
        Ok(activity)
    }

    async fn continue_item(
        &self,
        item: &WorkItem,
        screened: &ScreenedProposal,
        report: &CycleReport,
        run_id: &str,
        activity: &mut Activity,
    ) -> Result<(), WorkStateError> {
        let status = screened.proposal.status;
        let closing = match (status, item.created_by_reili) {
            (WorkItemStatus::Resolved, true) => ProgressClosing::Closing,
            (WorkItemStatus::Resolved, false) => ProgressClosing::SuggestClosing,
            _ => ProgressClosing::Continue,
        };
        let work_state = &self.deps.work_state;
        work_state
            .add_comment(
                &item.record,
                &render_progress_comment(screened, report, run_id, closing),
            )
            .await?;
        let needs_decision = status == WorkItemStatus::NeedsDecision;
        if needs_decision != item.needs_decision {
            work_state
                .set_needs_decision(&item.record, needs_decision)
                .await?;
        }
        if closing == ProgressClosing::Closing {
            work_state
                .close_work_item(&item.record, &CloseReason::Done)
                .await?;
        }

        let line = line(&item.record, &item.title, screened);
        if needs_decision && !item.needs_decision {
            activity.needs_decision.push(line.clone());
        }
        if status == WorkItemStatus::Resolved {
            activity.resolved.push(line.clone());
        }
        activity.updated.push(line);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_item(
        &self,
        objective: &ObjectiveDefinition,
        key: WorkItemKey,
        screened: &ScreenedProposal,
        body: String,
        report: &CycleReport,
        run_id: &str,
        activity: &mut Activity,
    ) -> Result<(), WorkStateError> {
        let needs_decision = screened.proposal.status == WorkItemStatus::NeedsDecision;
        let created = self
            .deps
            .work_state
            .create_work_item(&NewWorkItem {
                objective_id: objective.id.clone(),
                key: key.clone(),
                title: sanitize_for_storage(&screened.proposal.title, MAX_TITLE_CHARS),
                body,
                needs_decision,
            })
            .await?;

        // Another instance may have opened the same item at the same time; the oldest one wins.
        if let Some(canonical) = self
            .older_open_item_with_key(objective, &key, &created)
            .await?
        {
            self.deps
                .work_state
                .close_work_item(
                    &created.record,
                    &CloseReason::DuplicateOf(canonical.record.clone()),
                )
                .await?;
            if canonical.is_reili_work() {
                return self
                    .continue_item(&canonical, screened, report, run_id, activity)
                    .await;
            }
            return Ok(());
        }

        let line = line(&created.record, &created.title, screened);
        if needs_decision {
            activity.needs_decision.push(line.clone());
        }
        activity.created.push(line);
        Ok(())
    }

    async fn older_open_item_with_key(
        &self,
        objective: &ObjectiveDefinition,
        key: &WorkItemKey,
        created: &WorkItem,
    ) -> Result<Option<WorkItem>, WorkStateError> {
        let since = self.deps.clock.now() - Duration::days(DUPLICATE_LOOKBACK_DAYS);
        // Listing is in creation order, so the first open item with the key is the oldest.
        let oldest = self
            .deps
            .work_state
            .list_work_items(&objective.id, since)
            .await?
            .into_iter()
            .find(|item| item.state == WorkItemState::Open && item.key.as_ref() == Some(key));
        Ok(oldest.filter(|item| item.record != created.record))
    }
}

fn line(record: &RecordRef, title: &str, screened: &ScreenedProposal) -> ItemLine {
    let proposal = &screened.proposal;
    ItemLine {
        record: record.clone(),
        title: title.to_string(),
        severity: proposal.severity,
        question: (proposal.status == WorkItemStatus::NeedsDecision)
            .then(|| proposal.decision_needed.clone())
            .flatten(),
    }
}
