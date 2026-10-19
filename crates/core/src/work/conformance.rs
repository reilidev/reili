use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};

use super::item::{WorkItem, WorkItemState};
use super::port::WorkStatePort;
use super::record::RecordRef;
use super::test_support::sample_new_work_item;
use crate::objective::ObjectiveId;

/// A backend under test, plus the ability to act as a team member would in the tracker's own UI.
#[async_trait]
pub trait WorkStateBackendHarness: Send + Sync {
    fn repository(&self) -> Arc<dyn WorkStatePort>;

    /// Opens an item labelled for the objective, not assigned to Reili.
    async fn create_as_human(&self, objective: &ObjectiveId, title: &str, body: &str) -> RecordRef;

    /// Opens an item without any objective label, not assigned to Reili.
    async fn create_unlinked_as_human(&self, title: &str, body: &str) -> RecordRef;

    async fn comment_as_human(&self, record: &RecordRef, author: &str, trusted: bool, body: &str);

    async fn set_assigned_to_reili_as_human(&self, record: &RecordRef, assigned: bool);
}

/// Runs every contract check. Each check uses its own objective, so one backend instance is enough.
pub async fn assert_work_state_contract(harness: &dyn WorkStateBackendHarness) {
    created_items_are_labelled_and_assigned(harness).await;
    the_needs_decision_label_round_trips(harness).await;
    humans_assign_and_unassign_reili(harness).await;
    unlinked_reili_work_is_listed_until_linked(harness).await;
    comments_are_ordered_with_authorship(harness).await;
    closed_items_are_listed_until_they_age_out(harness).await;
}

fn objective_id(id: &str) -> ObjectiveId {
    ObjectiveId::new(id).unwrap_or_else(|_| unreachable!())
}

fn long_ago() -> DateTime<Utc> {
    Utc.timestamp_opt(0, 0).single().unwrap_or_default()
}

async fn list(repository: &Arc<dyn WorkStatePort>, objective: &str) -> Vec<WorkItem> {
    repository
        .list_work_items(&objective_id(objective), long_ago())
        .await
        .unwrap_or_else(|error| panic!("list work items of `{objective}`: {error}"))
}

async fn create(repository: &Arc<dyn WorkStatePort>, objective: &str, title: &str) -> WorkItem {
    repository
        .create_work_item(&sample_new_work_item(objective, title))
        .await
        .unwrap_or_else(|error| panic!("create work item `{title}`: {error}"))
}

fn find<'a>(items: &'a [WorkItem], record: &RecordRef) -> &'a WorkItem {
    items
        .iter()
        .find(|item| item.record == *record)
        .unwrap_or_else(|| panic!("{} is listed", record.key))
}

async fn created_items_are_labelled_and_assigned(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let first = create(&repository, "contract-create", "contract:first").await;
    let second = create(&repository, "contract-create", "contract:second").await;
    create(&repository, "contract-other", "contract:elsewhere").await;

    let items = list(&repository, "contract-create").await;

    let records: Vec<&RecordRef> = items.iter().map(|item| &item.record).collect();
    assert_eq!(
        records,
        vec![&first.record, &second.record],
        "items are listed oldest first and only for their objective"
    );
    let item = &items[0];
    assert_eq!(item.state, WorkItemState::Open);
    assert!(
        item.assigned_to_reili,
        "Reili's items carry the assignee label"
    );
    assert!(item.created_by_reili);
    assert!(!item.needs_decision);
    assert_eq!(item.title, "contract:first");
    assert_eq!(item.body.trim(), "sample problem");
}

async fn the_needs_decision_label_round_trips(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let created = create(&repository, "contract-decision", "contract:decision").await;

    repository
        .set_needs_decision(&created.record, true)
        .await
        .unwrap_or_else(|error| panic!("set needs-decision: {error}"));
    let marked = list(&repository, "contract-decision").await;
    repository
        .set_needs_decision(&created.record, false)
        .await
        .unwrap_or_else(|error| panic!("clear needs-decision: {error}"));
    let cleared = list(&repository, "contract-decision").await;

    assert!(find(&marked, &created.record).needs_decision);
    assert!(!find(&cleared, &created.record).needs_decision);
    assert!(
        find(&cleared, &created.record).assigned_to_reili,
        "other labels survive label changes"
    );
}

async fn humans_assign_and_unassign_reili(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let human = harness
        .create_as_human(
            &objective_id("contract-assign"),
            "Look at retries",
            "Please check",
        )
        .await;
    let reili = create(&repository, "contract-assign", "contract:own").await;

    let before = list(&repository, "contract-assign").await;
    harness.set_assigned_to_reili_as_human(&human, true).await;
    harness
        .set_assigned_to_reili_as_human(&reili.record, false)
        .await;
    let after = list(&repository, "contract-assign").await;

    let human_item = find(&after, &human);
    assert!(!find(&before, &human).assigned_to_reili);
    assert!(
        human_item.assigned_to_reili,
        "a human can hand an item to Reili"
    );
    assert!(!human_item.created_by_reili);
    assert_eq!(human_item.title, "Look at retries");
    assert!(
        !find(&after, &reili.record).assigned_to_reili,
        "a human can take an item away from Reili"
    );
}

async fn comments_are_ordered_with_authorship(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let created = create(&repository, "contract-comments", "contract:comments").await;
    repository
        .add_comment(&created.record, "Progress from Reili")
        .await
        .unwrap_or_else(|error| panic!("add comment: {error}"));
    harness
        .comment_as_human(&created.record, "alice", true, "first")
        .await;
    harness
        .comment_as_human(&created.record, "mallory", false, "second")
        .await;

    let comments = repository
        .list_comments(&created.record)
        .await
        .unwrap_or_else(|error| panic!("list comments: {error}"));

    let seen: Vec<(bool, bool, &str)> = comments
        .iter()
        .map(|comment| {
            (
                comment.by_reili,
                comment.author_trusted,
                comment.body.trim(),
            )
        })
        .collect();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].0, "Reili's own comment is recognised");
    assert!(seen[0].2.ends_with("Progress from Reili"));
    assert_eq!(seen[1], (false, true, "first"));
    assert_eq!(
        seen[2],
        (false, false, "second"),
        "untrusted authors are flagged"
    );
}

async fn closed_items_are_listed_until_they_age_out(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let open = create(&repository, "contract-close", "contract:open").await;
    let done = create(&repository, "contract-close", "contract:done").await;

    repository
        .close_work_item(&done.record)
        .await
        .unwrap_or_else(|error| panic!("close work item: {error}"));
    let recent = list(&repository, "contract-close").await;
    let future = open.created_at + Duration::days(365 * 100);
    let aged = repository
        .list_work_items(&objective_id("contract-close"), future)
        .await
        .unwrap_or_else(|error| panic!("list work items: {error}"));

    assert_eq!(find(&recent, &done.record).state, WorkItemState::Done);
    assert!(!find(&recent, &done.record).is_reili_work());
    let aged_records: Vec<&RecordRef> = aged.iter().map(|item| &item.record).collect();
    assert_eq!(
        aged_records,
        vec![&open.record],
        "open items are always listed; closed ones only after `closed_since`"
    );
}

async fn unlinked_reili_work_is_listed_until_linked(harness: &dyn WorkStateBackendHarness) {
    let repository = harness.repository();
    let assigned = harness
        .create_unlinked_as_human("Unlinked assigned", "assigned without an objective")
        .await;
    harness
        .set_assigned_to_reili_as_human(&assigned, true)
        .await;
    let unassigned = harness
        .create_unlinked_as_human("Unlinked unassigned", "nobody asked Reili")
        .await;
    let linked = harness
        .create_as_human(&objective_id("contract-linked"), "Linked", "already linked")
        .await;
    harness.set_assigned_to_reili_as_human(&linked, true).await;

    let before = repository
        .list_unlinked_reili_work()
        .await
        .unwrap_or_else(|error| panic!("list unlinked work: {error}"));
    repository
        .link_to_objective(&assigned, &objective_id("contract-link"))
        .await
        .unwrap_or_else(|error| panic!("link to objective: {error}"));
    let after = repository
        .list_unlinked_reili_work()
        .await
        .unwrap_or_else(|error| panic!("list unlinked work: {error}"));
    let items = list(&repository, "contract-link").await;

    assert!(
        before.contains(&assigned),
        "assigned unlinked items are listed"
    );
    assert!(
        !before.contains(&unassigned),
        "items not assigned to Reili are not listed"
    );
    assert!(!before.contains(&linked), "linked items are not listed");
    assert!(!after.contains(&assigned), "linking removes the item");
    assert!(
        find(&items, &assigned).assigned_to_reili,
        "the linked item keeps its assignee label"
    );
}
