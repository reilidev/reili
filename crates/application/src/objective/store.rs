//! Conflict-tolerant access to Work State. Backends offer no transactions, so every write is
//! a read-modify-write guarded by a revision number; on conflict the mutation is re-applied to the
//! freshly read state. Creates are followed by a re-listing to collapse racing duplicates.

use std::sync::Arc;

use reili_core::objective::{ObjectiveDefinition, ObjectiveId};
use reili_core::work::{
    ObjectiveLedgerState, StoredLedger, StoredWorkItem, WorkItemState, WorkStateError,
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

    /// Returns the objective's ledger, creating it when missing. If two instances created a ledger
    /// concurrently, the oldest record wins and the other is closed as a duplicate.
    pub async fn ensure_ledger(
        &self,
        objective: &ObjectiveDefinition,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<StoredLedger, WorkStateError> {
        if let Some(existing) = self.repository.find_objective_ledger(&objective.id).await? {
            return Ok(existing);
        }
        let created = self
            .repository
            .create_objective_ledger(&ObjectiveLedgerState::new(objective, now))
            .await?;
        let canonical = self
            .repository
            .find_objective_ledger(&objective.id)
            .await?
            .unwrap_or_else(|| created.clone());
        if canonical.record.key != created.record.key {
            self.repository
                .close_duplicate(&created.record, &canonical.record)
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
        F: FnMut(&mut ObjectiveLedgerState) -> R + Send,
        R: Send,
    {
        let mut current = ledger.clone();
        let mut last_error = None;
        for _ in 0..MAX_WRITE_ATTEMPTS {
            let mut next = current.state.clone();
            let result = mutate(&mut next);
            match self
                .repository
                .save_objective_ledger(&current.record, current.state.revision, &next)
                .await
            {
                Ok(saved) => return Ok((saved, result)),
                Err(error @ WorkStateError::Conflict { .. }) => {
                    last_error = Some(error);
                    current = self.reload_ledger(&current.state.objective_id).await?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            WorkStateError::Other(reili_core::error::PortError::new("ledger write failed"))
        }))
    }

    pub async fn reload_ledger(
        &self,
        objective_id: &ObjectiveId,
    ) -> Result<StoredLedger, WorkStateError> {
        self.repository
            .find_objective_ledger(objective_id)
            .await?
            .ok_or_else(|| {
                WorkStateError::Other(reili_core::error::PortError::new(format!(
                    "objective ledger for `{objective_id}` disappeared"
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
                .save_work_item(&current.record, current.state.revision, &next)
                .await
            {
                Ok(saved) => return Ok(saved),
                Err(error @ WorkStateError::Conflict { .. }) => {
                    last_error = Some(error);
                    current = self
                        .repository
                        .list_work_items(&current.state.objective_id)
                        .await?
                        .into_iter()
                        .find(|candidate| candidate.record.key == current.record.key)
                        .ok_or_else(|| {
                            WorkStateError::Other(reili_core::error::PortError::new(format!(
                                "work item {} disappeared",
                                current.record.key
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
    /// objective's items are listed again (the listing API is strongly consistent, unlike search) and
    /// racing duplicates are closed in favour of the oldest record.
    pub async fn create_work_item_once(
        &self,
        state: &WorkItemState,
    ) -> Result<StoredWorkItem, WorkStateError> {
        let existing = self.repository.list_work_items(&state.objective_id).await?;
        if let Some(found) = existing
            .into_iter()
            .find(|item| item.state.key == state.key)
        {
            return Ok(found);
        }
        let mut state = state.clone();
        state.enforce_limits();
        let created = self.repository.create_work_item(&state).await?;
        let same_key: Vec<StoredWorkItem> = self
            .repository
            .list_work_items(&state.objective_id)
            .await?
            .into_iter()
            .filter(|item| item.state.key == state.key)
            .collect();
        // Listing is in creation order, so the oldest record wins.
        let Some(canonical) = same_key.first().cloned() else {
            return Ok(created);
        };
        for duplicate in same_key.iter().skip(1) {
            self.repository
                .close_duplicate(&duplicate.record, &canonical.record)
                .await?;
        }
        Ok(canonical)
    }
}
