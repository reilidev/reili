use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    CycleReport, NewWorkItem, RecordRef, WorkItem, WorkItemProposal, WorkItemStatus,
    WorkStateError, text::truncate_chars,
};

use super::super::decide::{PlanChangesInput, PlannedChange, plan_changes};
use super::super::render::{
    ProgressClosing, RenderNewItemInput, render_new_item_body, render_progress_comment,
};
use super::super::report::{Activity, TouchedItem};
use super::ObjectiveCycleRunner;
use super::observe::Observed;

const MAX_TITLE_CHARS: usize = 200;

impl ObjectiveCycleRunner {
    pub(super) async fn apply(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        observed: &Observed,
        report: &CycleReport,
    ) -> Result<Activity, WorkStateError> {
        let mut activity = Activity::default();
        let changes = plan_changes(&PlanChangesInput {
            objective,
            items: &observed.items,
            proposals: &report.work_items,
        });
        for change in changes {
            match change {
                PlannedChange::Update { item, proposal } => {
                    self.continue_item(&item, &proposal, report, run_id, &mut activity)
                        .await?;
                }
                PlannedChange::Create {
                    proposal,
                    follows_up,
                } => {
                    let body = render_new_item_body(&RenderNewItemInput {
                        objective,
                        proposal: &proposal,
                        report,
                        follows_up: follows_up.as_ref(),
                        run_id,
                    });
                    self.open_item(objective, &proposal, body, &mut activity)
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
        proposal: &WorkItemProposal,
        report: &CycleReport,
        run_id: &str,
        activity: &mut Activity,
    ) -> Result<(), WorkStateError> {
        let status = proposal.status;
        let closing = match (status, item.created_by_reili) {
            (WorkItemStatus::Resolved, true) => ProgressClosing::Closing,
            (WorkItemStatus::Resolved, false) => ProgressClosing::SuggestClosing,
            _ => ProgressClosing::Continue,
        };
        let work_state = &self.deps.work_state;
        work_state
            .add_comment(
                &item.record,
                &render_progress_comment(proposal, report, run_id, closing),
            )
            .await?;
        let needs_decision = status == WorkItemStatus::NeedsDecision;
        if needs_decision != item.needs_decision {
            work_state
                .set_needs_decision(&item.record, needs_decision)
                .await?;
        }
        if closing == ProgressClosing::Closing {
            work_state.close_work_item(&item.record).await?;
        }

        activity.updated.push(touched(proposal, &item.record));
        Ok(())
    }

    async fn open_item(
        &self,
        objective: &ObjectiveDefinition,
        proposal: &WorkItemProposal,
        body: String,
        activity: &mut Activity,
    ) -> Result<(), WorkStateError> {
        let created = self
            .deps
            .work_state
            .create_work_item(&NewWorkItem {
                objective_id: objective.id.clone(),
                title: truncate_chars(proposal.title.trim(), MAX_TITLE_CHARS),
                body,
                needs_decision: proposal.status == WorkItemStatus::NeedsDecision,
            })
            .await?;
        activity.created.push(touched(proposal, &created.record));
        Ok(())
    }
}

fn touched(proposal: &WorkItemProposal, record: &RecordRef) -> TouchedItem {
    TouchedItem {
        key: proposal.key.clone(),
        record: record.clone(),
    }
}
