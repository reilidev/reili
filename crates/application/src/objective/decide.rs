use std::collections::BTreeSet;

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    RecordKey, RecordRef, WorkItem, WorkItemProposal, WorkItemState, WorkItemStatus,
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

#[must_use]
pub fn plan_changes(input: &PlanChangesInput<'_>) -> Vec<PlannedChange> {
    let mut changes = Vec::new();
    let mut creates = Vec::new();
    let mut claimed: BTreeSet<&RecordKey> = BTreeSet::new();
    for proposal in input.proposals {
        let skip = |reason: &str| PlannedChange::Skipped {
            title: proposal.title.clone(),
            reason: reason.to_string(),
        };
        let create = |follows_up: Option<RecordRef>| PlannedChange::Create {
            proposal: proposal.clone(),
            follows_up,
        };
        match resolve_existing_record(proposal, input.items) {
            ExistingRecord::Unknown(record) => {
                changes.push(skip(&format!("{record} is not an item of this objective")));
            }
            ExistingRecord::Found(item) if claimed.contains(&item.record.key) => {
                changes.push(skip("another proposal in this report already covers it"));
            }
            ExistingRecord::Found(item) if item.is_reili_work() => {
                claimed.insert(&item.record.key);
                changes.push(PlannedChange::Update {
                    item: Box::new(item.clone()),
                    proposal: proposal.clone(),
                });
            }
            ExistingRecord::Found(item) if item.state == WorkItemState::Open => {
                changes.push(skip(&format!(
                    "{} is not assigned to Reili; left to the team",
                    item.record.key
                )));
            }
            ExistingRecord::Found(item) if item.state == WorkItemState::NotPlanned => {
                changes.push(skip(&format!(
                    "the team closed {} as not planned",
                    item.record.key
                )));
            }
            ExistingRecord::Found(_) if proposal.status == WorkItemStatus::Resolved => {
                changes.push(skip("already done"));
            }
            ExistingRecord::Found(item) => creates.push(create(Some(item.record.clone()))),
            ExistingRecord::NotGiven if proposal.status == WorkItemStatus::Resolved => {
                changes.push(skip("nothing open to resolve"));
            }
            ExistingRecord::NotGiven => creates.push(create(None)),
        }
    }

    let limit =
        usize::try_from(input.objective.budget.max_new_work_items_per_cycle).unwrap_or(usize::MAX);
    for (index, change) in creates.into_iter().enumerate() {
        if index < limit {
            changes.push(change);
        } else if let PlannedChange::Create { proposal, .. } = change {
            changes.push(PlannedChange::Skipped {
                title: proposal.title,
                reason: format!("limit of {limit} new work item(s) per cycle reached"),
            });
        }
    }
    changes
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

        let changes = plan(
            &items,
            &[
                proposal_for(Some("#1"), WorkItemStatus::InProgress),
                proposal_for(Some("#2"), WorkItemStatus::InProgress),
                proposal_for(Some("#9"), WorkItemStatus::InProgress),
            ],
        );

        assert_eq!(changes.len(), 3);
        assert!(
            changes
                .iter()
                .all(|change| matches!(change, PlannedChange::Skipped { .. }))
        );
    }

    #[test]
    fn follows_up_on_done_items_and_limits_new_ones_in_report_order() {
        let items = vec![item(1, WorkItemState::Done, false)];
        let mut objective = sample_objective("checkout");
        objective.budget.max_new_work_items_per_cycle = 2;
        let mut first = proposal_for(None, WorkItemStatus::InProgress);
        first.title = "first".to_string();
        let mut beyond_limit = proposal_for(None, WorkItemStatus::InProgress);
        beyond_limit.title = "beyond limit".to_string();
        let mut recurring = proposal_for(Some("#1"), WorkItemStatus::InProgress);
        recurring.title = "recurring".to_string();

        let changes = plan_changes(&PlanChangesInput {
            objective: &objective,
            items: &items,
            proposals: &[first, recurring, beyond_limit],
        });

        let created: Vec<&str> = changes
            .iter()
            .filter_map(|change| match change {
                PlannedChange::Create { proposal, .. } => Some(proposal.title.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(created, vec!["first", "recurring"]);
        assert!(changes.iter().any(|change| matches!(
            change,
            PlannedChange::Create { follows_up: Some(record), .. } if record.key.as_str() == "#1"
        )));
        assert!(changes.iter().any(|change| matches!(
            change,
            PlannedChange::Skipped { reason, .. } if reason.contains("limit")
        )));
    }

    #[test]
    fn continues_an_item_once_when_two_proposals_name_it() {
        let items = vec![item(1, WorkItemState::Open, true)];

        let changes = plan(
            &items,
            &[
                proposal_for(Some("#1"), WorkItemStatus::InProgress),
                proposal_for(Some(" #1 "), WorkItemStatus::NeedsDecision),
            ],
        );

        assert!(matches!(
            &changes[..],
            [PlannedChange::Update { .. }, PlannedChange::Skipped { reason, .. }]
                if reason.contains("already covers")
        ));
    }

    #[test]
    fn does_not_open_items_only_to_resolve_them() {
        let changes = plan(&[], &[proposal_for(None, WorkItemStatus::Resolved)]);

        assert!(matches!(&changes[..], [PlannedChange::Skipped { .. }]));
    }
}
