//! Conflict-tolerant access to Work State. GitHub Issues offer no transactions, so every write is
//! a read-modify-write guarded by a revision number; on conflict the mutation is re-applied to the
//! freshly read state. Creates are followed by a re-listing to collapse racing duplicates.

use std::sync::Arc;

use reili_core::goal::{GoalDefinition, GoalId};
use reili_core::work::{
    GoalLedgerState, StoredLedger, StoredWorkItem, WorkItemState, WorkStateError,
    WorkStateRepositoryPort,
};

const MAX_WRITE_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub struct WorkStateStore {
    repository: Arc<dyn WorkStateRepositoryPort>,
}

impl WorkStateStore {
    pub fn new(repository: Arc<dyn WorkStateRepositoryPort>) -> Self {
        Self { repository }
    }

    #[must_use]
    pub fn repository(&self) -> &Arc<dyn WorkStateRepositoryPort> {
        &self.repository
    }

    /// Returns the goal's ledger, creating it when missing. If two instances created a ledger
    /// concurrently, the lowest issue number wins and the other is closed as a duplicate.
    pub async fn ensure_ledger(
        &self,
        goal: &GoalDefinition,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<StoredLedger, WorkStateError> {
        if let Some(existing) = self.repository.find_goal_ledger(&goal.id).await? {
            return Ok(existing);
        }
        let created = self
            .repository
            .create_goal_ledger(&GoalLedgerState::new(goal, now))
            .await?;
        let canonical = self
            .repository
            .find_goal_ledger(&goal.id)
            .await?
            .unwrap_or_else(|| created.clone());
        if canonical.issue.number != created.issue.number {
            self.repository
                .close_duplicate(&created.issue, &canonical.issue)
                .await?;
        }
        Ok(canonical)
    }

    /// Applies `mutate` to the ledger and saves it, re-reading and re-applying on conflict.
    pub async fn mutate_ledger<F, R>(
        &self,
        ledger: &StoredLedger,
        mut mutate: F,
    ) -> Result<(StoredLedger, R), WorkStateError>
    where
        F: FnMut(&mut GoalLedgerState) -> R + Send,
        R: Send,
    {
        let mut current = ledger.clone();
        let mut last_error = None;
        for _ in 0..MAX_WRITE_ATTEMPTS {
            let mut next = current.state.clone();
            let result = mutate(&mut next);
            match self
                .repository
                .save_goal_ledger(&current.issue, current.state.revision, &next)
                .await
            {
                Ok(saved) => return Ok((saved, result)),
                Err(error @ WorkStateError::Conflict { .. }) => {
                    last_error = Some(error);
                    current = self.reload_ledger(&current.state.goal_id).await?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            WorkStateError::Other(reili_core::error::PortError::new("ledger write failed"))
        }))
    }

    pub async fn reload_ledger(&self, goal_id: &GoalId) -> Result<StoredLedger, WorkStateError> {
        self.repository
            .find_goal_ledger(goal_id)
            .await?
            .ok_or_else(|| {
                WorkStateError::Other(reili_core::error::PortError::new(format!(
                    "goal ledger for `{goal_id}` disappeared"
                )))
            })
    }

    /// Applies `mutate` to a work item and saves it, re-reading and re-applying on conflict.
    pub async fn mutate_work_item<F>(
        &self,
        item: &StoredWorkItem,
        mut mutate: F,
    ) -> Result<StoredWorkItem, WorkStateError>
    where
        F: FnMut(&mut WorkItemState) + Send,
    {
        let mut current = item.clone();
        let mut last_error = None;
        for _ in 0..MAX_WRITE_ATTEMPTS {
            let mut next = current.state.clone();
            mutate(&mut next);
            next.enforce_limits();
            match self
                .repository
                .save_work_item(&current.issue, current.state.revision, &next)
                .await
            {
                Ok(saved) => return Ok(saved),
                Err(error @ WorkStateError::Conflict { .. }) => {
                    last_error = Some(error);
                    current = self
                        .repository
                        .list_work_items(&current.state.goal_id)
                        .await?
                        .into_iter()
                        .find(|candidate| candidate.issue.number == current.issue.number)
                        .ok_or_else(|| {
                            WorkStateError::Other(reili_core::error::PortError::new(format!(
                                "work item #{} disappeared",
                                current.issue.number
                            )))
                        })?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            WorkStateError::Other(reili_core::error::PortError::new("work item write failed"))
        }))
    }

    /// Creates a work item unless one with the same key already exists. After creating, the
    /// goal's items are listed again (the listing API is strongly consistent, unlike search) and
    /// racing duplicates are closed in favour of the lowest issue number.
    pub async fn create_work_item_once(
        &self,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let existing = self.repository.list_work_items(&state.goal_id).await?;
        if let Some(found) = existing
            .into_iter()
            .find(|item| item.state.key == state.key)
        {
            return Ok(found);
        }
        let mut state = state.clone();
        state.enforce_limits();
        let created = self.repository.create_work_item(&state).await?;
        let mut same_key: Vec<StoredWorkItem> = self
            .repository
            .list_work_items(&state.goal_id)
            .await?
            .into_iter()
            .filter(|item| item.state.key == state.key)
            .collect();
        same_key.sort_by_key(|item| item.issue.number);
        let Some(canonical) = same_key.first().cloned() else {
            return Ok(created);
        };
        for duplicate in same_key.iter().skip(1) {
            self.repository
                .close_duplicate(&duplicate.issue, &canonical.issue)
                .await?;
        }
        Ok(canonical)
    }
}
