use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    MAX_WORK_ITEMS_PER_CYCLE, RecordRef, WorkItem, WorkItemProposal, WorkItemState, WorkItemStatus,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PlannedChange {
    Update {
        item: Box<WorkItem>,
        proposal: WorkItemProposal,
    },
    Create {
        proposal: WorkItemProposal,
        /// A done item that this one follows up on.
        follows_up: Option<RecordRef>,
    },
    Skipped {
        title: String,
        reason: String,
    },
}

pub struct PlanChangesInput<'a> {
    pub objective: &'a ObjectiveDefinition,
    pub items: &'a [WorkItem],
    pub proposals: &'a [WorkItemProposal],
}

/// What a proposal's `existing_record` points to among the objective's items.
enum ExistingRecord<'a> {
    NotGiven,
    Unknown(&'a str),
    Found(&'a WorkItem),
}

fn resolve_existing_record<'a>(
    proposal: &'a WorkItemProposal,
    items: &'a [WorkItem],
) -> ExistingRecord<'a> {
    let Some(record) = proposal
        .existing_record
        .as_deref()
        .map(str::trim)
        .filter(|record| !record.is_empty())
    else {
        return ExistingRecord::NotGiven;
    };
    items
        .iter()
        .find(|item| item.record.key.as_str() == record)
        .map_or(ExistingRecord::Unknown(record), ExistingRecord::Found)
}

/// Plans the report's first proposal, this cycle's one focus; the others wait for later cycles.
#[must_use]
pub fn plan_changes(input: &PlanChangesInput<'_>) -> Vec<PlannedChange> {
    let (focus, rest) = input
        .proposals
        .split_at(input.proposals.len().min(MAX_WORK_ITEMS_PER_CYCLE));
    let limit =
        usize::try_from(input.objective.budget.max_new_work_items_per_cycle).unwrap_or(usize::MAX);
    let mut created = 0;
    let mut changes = Vec::new();
    for proposal in focus {
        let change = plan_change(proposal, input.items);
        if !matches!(change, PlannedChange::Create { .. }) {
            changes.push(change);
        } else if created < limit {
            created += 1;
            changes.push(change);
        } else {
            changes.push(skipped(
                proposal,
                &format!("limit of {limit} new work item(s) per cycle reached"),
            ));
        }
    }
    changes.extend(rest.iter().map(|proposal| {
        skipped(
            proposal,
            &format!("a cycle works on {MAX_WORK_ITEMS_PER_CYCLE} item(s); left for a later cycle"),
        )
    }));
    changes
}

fn plan_change(proposal: &WorkItemProposal, items: &[WorkItem]) -> PlannedChange {
    let create = |follows_up: Option<RecordRef>| PlannedChange::Create {
        proposal: proposal.clone(),
        follows_up,
    };
    match resolve_existing_record(proposal, items) {
        ExistingRecord::Unknown(record) => skipped(
            proposal,
            &format!("{record} is not an item of this objective"),
        ),
        ExistingRecord::Found(item) if item.is_reili_work() => PlannedChange::Update {
            item: Box::new(item.clone()),
            proposal: proposal.clone(),
        },
        ExistingRecord::Found(item) if item.state == WorkItemState::Open => skipped(
            proposal,
            &format!(
                "{} is not assigned to Reili; left to the team",
                item.record.key
            ),
        ),
        ExistingRecord::Found(item) if item.state == WorkItemState::NotPlanned => skipped(
            proposal,
            &format!("the team closed {} as not planned", item.record.key),
        ),
        ExistingRecord::Found(_) if proposal.status == WorkItemStatus::Resolved => {
            skipped(proposal, "already done")
        }
        ExistingRecord::Found(item) => create(Some(item.record.clone())),
        ExistingRecord::NotGiven if proposal.status == WorkItemStatus::Resolved => {
            skipped(proposal, "nothing open to resolve")
        }
        ExistingRecord::NotGiven => create(None),
    }
}

fn skipped(proposal: &WorkItemProposal, reason: &str) -> PlannedChange {
    PlannedChange::Skipped {
        title: proposal.title.clone(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use reili_core::work::{
        RecordKey, RecordRef, WorkItem, WorkItemProposal, WorkItemState, WorkItemStatus,
    };

    use super::{PlanChangesInput, PlannedChange, plan_changes};
    use crate::objective::testing::at;
    use crate::objective::tests_support::{proposal, sample_objective};

    fn item(number: u64, state: WorkItemState, assigned: bool) -> WorkItem {
        WorkItem {
            record: RecordRef {
                key: RecordKey::new(format!("#{number}")),
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
            objective_id: reili_core::objective::ObjectiveId::new("checkout").expect("id"),
            title: format!("Item {number}"),
            body: String::new(),
            author: "reili[bot]".to_string(),
            created_by_reili: true,
            state,
            assigned_to_reili: assigned,
            needs_decision: false,
            created_at: at("2026-10-15T00:00:00Z"),
            updated_at: at("2026-10-15T00:00:00Z"),
        }
    }

    fn proposal_for(record: Option<&str>, status: WorkItemStatus) -> WorkItemProposal {
        let mut proposal = proposal();
        proposal.existing_record = record.map(str::to_string);
        proposal.status = status;
        proposal
    }

    fn plan(items: &[WorkItem], proposals: &[WorkItemProposal]) -> Vec<PlannedChange> {
        plan_changes(&PlanChangesInput {
            objective: &sample_objective("checkout"),
            items,
            proposals,
        })
    }

    #[test]
    fn continues_own_items_named_by_record() {
        let items = vec![item(1, WorkItemState::Open, true)];

        let changes = plan(
            &items,
            &[proposal_for(Some("#1"), WorkItemStatus::InProgress)],
        );

        assert!(matches!(
            &changes[..],
            [PlannedChange::Update { item, .. }] if item.record.key.as_str() == "#1"
        ));
    }

    #[test]
    fn respects_the_teams_decisions_and_unknown_records() {
        let items = vec![
            item(1, WorkItemState::Open, false),
            item(2, WorkItemState::NotPlanned, false),
        ];

        for record in ["#1", "#2", "#9"] {
            let changes = plan(
                &items,
                &[proposal_for(Some(record), WorkItemStatus::InProgress)],
            );

            assert!(
                matches!(&changes[..], [PlannedChange::Skipped { .. }]),
                "{record} is left alone"
            );
        }
    }

    #[test]
    fn follows_up_on_done_items_with_a_new_one() {
        let items = vec![item(1, WorkItemState::Done, false)];

        let changes = plan(
            &items,
            &[proposal_for(Some("#1"), WorkItemStatus::InProgress)],
        );

        assert!(matches!(
            &changes[..],
            [PlannedChange::Create { follows_up: Some(record), .. }] if record.key.as_str() == "#1"
        ));
    }

    #[test]
    fn opens_no_new_items_when_the_objective_allows_none() {
        let mut objective = sample_objective("checkout");
        objective.budget.max_new_work_items_per_cycle = 0;

        let changes = plan_changes(&PlanChangesInput {
            objective: &objective,
            items: &[],
            proposals: &[proposal_for(None, WorkItemStatus::InProgress)],
        });

        assert!(matches!(
            &changes[..],
            [PlannedChange::Skipped { reason, .. }] if reason.contains("limit")
        ));
    }

    #[test]
    fn works_only_on_the_first_proposal() {
        let items = vec![
            item(1, WorkItemState::Open, true),
            item(2, WorkItemState::Open, true),
        ];

        let changes = plan(
            &items,
            &[
                proposal_for(Some("#1"), WorkItemStatus::InProgress),
                proposal_for(Some("#2"), WorkItemStatus::InProgress),
                proposal_for(None, WorkItemStatus::InProgress),
            ],
        );

        assert!(matches!(
            &changes[..],
            [
                PlannedChange::Update { item, .. },
                PlannedChange::Skipped { reason: second, .. },
                PlannedChange::Skipped { reason: third, .. },
            ] if item.record.key.as_str() == "#1"
                && second.contains("later cycle")
                && third.contains("later cycle")
        ));
    }

    #[test]
    fn does_not_open_items_only_to_resolve_them() {
        let changes = plan(&[], &[proposal_for(None, WorkItemStatus::Resolved)]);

        assert!(matches!(&changes[..], [PlannedChange::Skipped { .. }]));
    }
}
