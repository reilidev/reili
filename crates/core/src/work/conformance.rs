//! Contract every [`WorkStateRepositoryPort`] backend must satisfy (`test-support` feature).
//! Each backend's tests run [`assert_work_state_contract`] against it, so the guarantees the
//! objective cycle relies on are checked by code rather than only described in the port docs.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};

use super::item::{WorkItemState, WorkItemStatus};
use super::ledger::ObjectiveLedgerState;
use super::record::RecordRef;
use super::repository::{
    StoredWorkItem, WorkStateComment, WorkStateCommentKind, WorkStateError, WorkStateRepositoryPort,
};
use super::test_support::{sample_objective, sample_work_item};
use crate::objective::ObjectiveId;

/// A backend under test, plus the ability to act as a person would in the backend's own UI.
#[async_trait]
pub trait WorkStateBackendHarness: Send + Sync {
    fn repository(&self) -> Arc<dyn WorkStateRepositoryPort>;

    async fn comment_as_human(&self, record: &RecordRef, author: &str, trusted: bool, body: &str);

    async fn set_status_as_human(&self, record: &RecordRef, status: WorkItemStatus);
}

/// Runs every contract check. Each check uses its own objective, so one backend instance is enough.
pub async fn assert_work_state_contract(harness: &dyn WorkStateBackendHarness) {
    ledgers_round_trip_with_revision_checks(harness).await;
    work_items_are_listed_per_objective_in_creation_order(harness).await;
    work_item_writes_check_the_revision(harness).await;
    closed_duplicates_leave_the_listings(harness).await;
    statuses_written_by_reili_and_by_humans_are_observed(harness).await;
    human_comments_are_filtered_ordered_and_resumable(harness).await;
}

fn objective_id(id: &str) -> ObjectiveId {
    ObjectiveId::new(id).unwrap_or_else(|_| unreachable!())
}

fn item_for(objective: &str, key: &str) -> WorkItemState {
    let mut state = sample_work_item(key);
    state.objective_id = objective_id(objective);
    state
}

async fn list_items(
    repository: &Arc<dyn WorkStateRepositoryPort>,
    objective: &str,
) -> Vec<StoredWorkItem> {
    repository
        .list_work_items(&objective_id(objective))
        .await
        .unwrap_or_else(|error| panic!("list work items of `{objective}`: {error}"))
}

async fn create_item(
    repository: &Arc<dyn WorkStateRepositoryPort>,
    objective: &str,
    key: &str,
) -> StoredWorkItem {
    repository
        .create_work_item(&item_for(objective, key))
        .await
        .unwrap_or_else(|error| panic!("create work item `{key}`: {error}"))
}

async fn ledgers_round_trip_with_revision_checks(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let objective = sample_objective("contract-ledger");
    let now = Utc
        .timestamp_opt(1_760_000_000, 0)
        .single()
        .unwrap_or_default();

    let missing = repository
        .find_objective_ledger(&objective.id)
        .await
        .unwrap_or_else(|error| panic!("find missing ledger: {error}"));
    assert!(missing.is_none(), "no ledger exists before it is created");

    let created = repository
        .create_objective_ledger(&ObjectiveLedgerState::new(&objective, now))
        .await
        .unwrap_or_else(|error| panic!("create ledger: {error}"));
    assert_eq!(
        created.state.revision, 1,
        "a new ledger starts at revision 1"
    );

    let found = repository
        .find_objective_ledger(&objective.id)
        .await
        .unwrap_or_else(|error| panic!("find ledger: {error}"))
        .unwrap_or_else(|| panic!("created ledger is found immediately"));
    assert_eq!(found.record, created.record);
    assert_eq!(found.state, created.state);

    let listed = repository
        .list_objective_ledgers()
        .await
        .unwrap_or_else(|error| panic!("list ledgers: {error}"));
    assert!(
        listed.iter().any(|ledger| ledger.record == created.record),
        "created ledger is listed"
    );

    let mut next = created.state.clone();
    next.reports.recent_report_keys.push("contract".to_string());
    let saved = repository
        .save_objective_ledger(&created.record, 1, &next)
        .await
        .unwrap_or_else(|error| panic!("save ledger: {error}"));
    assert_eq!(saved.state.revision, 2, "each save increments the revision");
    assert_eq!(saved.state.reports.recent_report_keys, vec!["contract"]);

    let stale = repository
        .save_objective_ledger(&created.record, 1, &next)
        .await;
    assert!(
        matches!(stale, Err(WorkStateError::Conflict { .. })),
        "a save with a stale revision is a conflict, got {stale:?}"
    );
}

async fn work_items_are_listed_per_objective_in_creation_order(
    harness: &dyn WorkStateBackendHarness,
) {
    let repository = harness.repository();
    for key in ["contract:first", "contract:second", "contract:third"] {
        create_item(&repository, "contract-order", key).await;
    }
    create_item(&repository, "contract-other", "contract:elsewhere").await;

    let keys: Vec<String> = list_items(&repository, "contract-order")
        .await
        .iter()
        .map(|item| item.state.key.as_str().to_string())
        .collect();
    assert_eq!(
        keys,
        vec!["contract:first", "contract:second", "contract:third"],
        "work items are listed oldest first and only for their objective"
    );
}

async fn work_item_writes_check_the_revision(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let created = create_item(&repository, "contract-revision", "contract:revision").await;
    assert_eq!(
        created.state.revision, 1,
        "a new work item starts at revision 1"
    );

    let mut next = created.state.clone();
    next.title = "Renamed by the contract".to_string();
    let saved = repository
        .save_work_item(&created.record, 1, &next)
        .await
        .unwrap_or_else(|error| panic!("save work item: {error}"));
    assert_eq!(saved.record, created.record);
    assert_eq!(saved.state.revision, 2);
    assert_eq!(saved.state.title, "Renamed by the contract");

    let stale = repository.save_work_item(&created.record, 1, &next).await;
    assert!(
        matches!(stale, Err(WorkStateError::Conflict { .. })),
        "a save with a stale revision is a conflict, got {stale:?}"
    );
}

async fn closed_duplicates_leave_the_listings(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let canonical = create_item(&repository, "contract-duplicate", "contract:same").await;
    let duplicate = create_item(&repository, "contract-duplicate", "contract:same").await;
    assert_ne!(canonical.record.key, duplicate.record.key);

    repository
        .close_duplicate(&duplicate.record, &canonical.record)
        .await
        .unwrap_or_else(|error| panic!("close duplicate: {error}"));

    let records: Vec<RecordRef> = list_items(&repository, "contract-duplicate")
        .await
        .into_iter()
        .map(|item| item.record)
        .collect();
    assert_eq!(
        records,
        vec![canonical.record],
        "a closed duplicate is no longer listed"
    );
}

async fn statuses_written_by_reili_and_by_humans_are_observed(
    harness: &dyn WorkStateBackendHarness,
) {
    let repository = harness.repository();
    let created = create_item(&repository, "contract-status", "contract:status").await;
    assert_eq!(created.observed_status, WorkItemStatus::Investigating);

    let mut resolved = created.state.clone();
    resolved.status = WorkItemStatus::Resolved;
    let saved = repository
        .save_work_item(&created.record, created.state.revision, &resolved)
        .await
        .unwrap_or_else(|error| panic!("save resolved work item: {error}"));
    assert_eq!(
        saved.observed_status,
        WorkItemStatus::Resolved,
        "a status written by Reili is observed as written"
    );

    harness
        .set_status_as_human(&saved.record, WorkItemStatus::NeedsHuman)
        .await;
    let observed = list_items(&repository, "contract-status").await;
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0].state.status,
        WorkItemStatus::Resolved,
        "a human status change does not rewrite Reili's state"
    );
    assert_eq!(
        observed[0].observed_status,
        WorkItemStatus::NeedsHuman,
        "a human status change is observed"
    );
}

async fn human_comments_are_filtered_ordered_and_resumable(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let created = create_item(&repository, "contract-comments", "contract:comments").await;
    repository
        .append_comment(
            &created.record,
            WorkStateComment {
                kind: WorkStateCommentKind::CycleLog,
                body: "Written by Reili".to_string(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("append comment: {error}"));
    harness
        .comment_as_human(&created.record, "alice", true, "first")
        .await;
    harness
        .comment_as_human(&created.record, "mallory", false, "second")
        .await;

    let comments = repository
        .list_human_comments(&created.record, None)
        .await
        .unwrap_or_else(|error| panic!("list comments: {error}"));
    let seen: Vec<(&str, bool, &str)> = comments
        .iter()
        .map(|comment| {
            (
                comment.author.as_str(),
                comment.author_trusted,
                comment.body.as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![("alice", true, "first"), ("mallory", false, "second")],
        "only human comments, oldest first, with trust detected"
    );

    let newer = repository
        .list_human_comments(&created.record, Some(comments[0].id.clone()))
        .await
        .unwrap_or_else(|error| panic!("list newer comments: {error}"));
    assert_eq!(
        newer
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        vec!["second"],
        "a comment id works as a cursor for newer comments"
    );
}
