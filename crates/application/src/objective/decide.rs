//! Decide: map screened proposals onto the objective's work items, the way a teammate would. Reili
//! continues only items assigned to it, leaves items assigned away to the team, never reopens
//! work the team closed as not planned, and opens a limited number of new items.

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{RecordRef, WorkItem, WorkItemKey, WorkItemState, WorkItemStatus};

use super::policy::ScreenedProposal;

#[derive(Debug, Clone, PartialEq)]
pub enum PlannedChange {
    Update {
        item: Box<WorkItem>,
        proposal: ScreenedProposal,
    },
    Create {
        key: WorkItemKey,
        proposal: ScreenedProposal,
        /// A done item for the same problem that this one follows up on.
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
    pub proposals: &'a [ScreenedProposal],
}

#[must_use]
pub fn plan_changes(input: &PlanChangesInput<'_>) -> Vec<PlannedChange> {
    let mut changes = Vec::new();
    let mut creates = Vec::new();
    let mut claimed: Vec<&RecordRef> = Vec::new();
    for screened in input.proposals {
        let proposal = &screened.proposal;
        let skip = |reason: &str| PlannedChange::Skipped {
            title: proposal.title.clone(),
            reason: reason.to_string(),
        };
        let Some(key) = WorkItemKey::normalize(&proposal.key) else {
            changes.push(skip("the proposal has no usable key"));
            continue;
        };
        match find_match(input.items, proposal.existing_record.as_deref(), &key) {
            Some(item) if claimed.contains(&&item.record) => {
                changes.push(skip("another proposal in this report already covers it"));
            }
            Some(item) if item.is_reili_work() => {
                claimed.push(&item.record);
                changes.push(PlannedChange::Update {
                    item: Box::new(item.clone()),
                    proposal: screened.clone(),
                });
            }
            Some(item) if item.state == WorkItemState::Open => {
                changes.push(skip(&format!(
                    "{} is not assigned to Reili; left to the team",
                    item.record.key
                )));
            }
            Some(item) if item.state == WorkItemState::NotPlanned => {
                changes.push(skip(&format!(
                    "the team closed {} as not planned",
                    item.record.key
                )));
            }
            Some(_) if proposal.status == WorkItemStatus::Resolved => {
                changes.push(skip("already done"));
            }
            matched => {
                if proposal.status == WorkItemStatus::Resolved {
                    changes.push(skip("nothing open to resolve"));
                    continue;
                }
                creates.push(PlannedChange::Create {
                    key,
                    proposal: screened.clone(),
                    follows_up: matched.map(|item| item.record.clone()),
                });
            }
        }
    }

    creates.sort_by_key(|change| match change {
        PlannedChange::Create { proposal, .. } => proposal.proposal.severity,
        _ => reili_core::work::Severity::Low,
    });
    let limit = input.objective.budget.max_new_work_items_per_cycle as usize;
    for (index, change) in creates.into_iter().enumerate() {
        if index < limit {
            changes.push(change);
        } else if let PlannedChange::Create { proposal, .. } = change {
            changes.push(PlannedChange::Skipped {
                title: proposal.proposal.title,
                reason: format!("limit of {limit} new work item(s) per cycle reached"),
            });
        }
    }
    changes
}

/// The explicit record reference wins; otherwise the newest item with the same key.
fn find_match<'a>(
    items: &'a [WorkItem],
    existing_record: Option<&str>,
    key: &WorkItemKey,
) -> Option<&'a WorkItem> {
    if let Some(record) = existing_record
        .map(str::trim)
        .filter(|record| !record.is_empty())
        && let Some(found) = items.iter().find(|item| item.record.key.as_str() == record)
    {
        return Some(found);
    }
    items
        .iter()
        .rev()
        .find(|item| item.key.as_ref() == Some(key))
}

#[cfg(test)]
mod tests {
    use reili_core::work::{
        HypothesisStatus, RecordKey, RecordRef, Severity, WorkItem, WorkItemKey, WorkItemState,
        WorkItemStatus,
    };

    use super::{PlanChangesInput, PlannedChange, plan_changes};
    use crate::objective::policy::ScreenedProposal;
    use crate::objective::policy::tests::proposal;
    use crate::objective::testing::at;
    use crate::objective::tests_support::sample_objective;

    fn item(number: u64, key: Option<&str>, state: WorkItemState, assigned: bool) -> WorkItem {
        WorkItem {
            record: RecordRef {
                key: RecordKey::new(format!("#{number}")),
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
            objective_id: reili_core::objective::ObjectiveId::new("checkout").expect("id"),
            title: format!("Item {number}"),
            body: String::new(),
            author: "reili[bot]".to_string(),
            created_by_reili: key.is_some(),
            key: key.and_then(WorkItemKey::normalize),
            state,
            assigned_to_reili: assigned,
            needs_decision: false,
            created_at: at("2026-10-15T00:00:00Z"),
            updated_at: at("2026-10-15T00:00:00Z"),
        }
    }

    fn screened(key: &str, status: WorkItemStatus) -> ScreenedProposal {
        let mut proposal = proposal(HypothesisStatus::Proposed);
        proposal.key = key.to_string();
        proposal.status = status;
        ScreenedProposal {
            proposal,
            notes: Vec::new(),
        }
    }

    fn plan(items: &[WorkItem], proposals: &[ScreenedProposal]) -> Vec<PlannedChange> {
        plan_changes(&PlanChangesInput {
            objective: &sample_objective("checkout"),
            items,
            proposals,
        })
    }

    #[test]
    fn continues_own_items_by_key_or_record() {
        let items = vec![
            item(1, Some("checkout:5xx"), WorkItemState::Open, true),
            item(2, None, WorkItemState::Open, true),
        ];
        let mut by_record = screened("anything", WorkItemStatus::InProgress);
        by_record.proposal.existing_record = Some("#2".to_string());

        let changes = plan(
            &items,
            &[
                screened("checkout:5xx", WorkItemStatus::InProgress),
                by_record,
            ],
        );

        assert!(matches!(
            &changes[..],
            [PlannedChange::Update { item: first, .. }, PlannedChange::Update { item: second, .. }]
                if first.record.key.as_str() == "#1" && second.record.key.as_str() == "#2"
        ));
    }

    #[test]
    fn respects_the_teams_decisions() {
        let items = vec![
            item(1, Some("checkout:taken"), WorkItemState::Open, false),
            item(
                2,
                Some("checkout:rejected"),
                WorkItemState::NotPlanned,
                false,
            ),
        ];

        let changes = plan(
            &items,
            &[
                screened("checkout:taken", WorkItemStatus::InProgress),
                screened("checkout:rejected", WorkItemStatus::InProgress),
            ],
        );

        assert!(
            changes
                .iter()
                .all(|change| matches!(change, PlannedChange::Skipped { .. }))
        );
    }

    #[test]
    fn follows_up_on_done_items_and_limits_new_ones() {
        let items = vec![item(
            1,
            Some("checkout:recurring"),
            WorkItemState::Done,
            false,
        )];
        let mut objective = sample_objective("checkout");
        objective.budget.max_new_work_items_per_cycle = 2;
        let mut critical = screened("checkout:critical", WorkItemStatus::InProgress);
        critical.proposal.severity = Severity::Critical;
        let mut low = screened("checkout:low", WorkItemStatus::InProgress);
        low.proposal.severity = Severity::Low;

        let changes = plan_changes(&PlanChangesInput {
            objective: &objective,
            items: &items,
            proposals: &[
                low,
                screened("checkout:recurring", WorkItemStatus::InProgress),
                critical,
            ],
        });

        let created: Vec<&str> = changes
            .iter()
            .filter_map(|change| match change {
                PlannedChange::Create { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(created, vec!["checkout:critical", "checkout:recurring"]);
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
    fn does_not_open_items_only_to_resolve_them() {
        let changes = plan(&[], &[screened("checkout:gone", WorkItemStatus::Resolved)]);

        assert!(matches!(&changes[..], [PlannedChange::Skipped { .. }]));
    }
}
