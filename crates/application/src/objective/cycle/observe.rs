//! Observe: load the objective's work items, fold in human changes, and read Canvas memory.

use reili_core::messaging::slack::{ListSlackCanvasMemoriesInput, SlackCanvasMemoryRecord};
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{StoredLedger, StoredWorkItem};

use super::super::reconcile::reconcile_human_signals;
use super::{ObjectiveCycleError, ObjectiveCycleRunner};

const MAX_MEMORIES_IN_BRIEF: u32 = 15;

pub(super) struct ObserveOutput {
    pub(super) items: Vec<StoredWorkItem>,
    pub(super) memories: Vec<SlackCanvasMemoryRecord>,
    pub(super) notes: Vec<String>,
}

impl ObjectiveCycleRunner {
    pub(super) async fn observe(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
    ) -> Result<ObserveOutput, ObjectiveCycleError> {
        let now = self.deps.clock.now();
        let items = self
            .deps
            .store
            .repository()
            .list_work_items(&objective.id)
            .await?;
        let reconciled = reconcile_human_signals(&self.deps.store, items, now).await?;
        let mut notes = reconciled.notes;

        let items = reconciled.items;

        let memories = match &self.deps.task_resources.canvas_memory_port {
            Some(port) => match port
                .list_channel_memories(ListSlackCanvasMemoriesInput {
                    channel_id: objective.report.channel_id.clone(),
                    channel_name: None,
                    limit: MAX_MEMORIES_IN_BRIEF,
                })
                .await
            {
                Ok(memories) => memories,
                Err(error) => {
                    notes.push(format!("Canvas memory unavailable: {}", error.message));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        if ledger.state.definition_fingerprint != objective.fingerprint() {
            notes.push("objective definition fingerprint differs from the ledger".to_string());
        }

        Ok(ObserveOutput {
            items,
            memories,
            notes,
        })
    }
}
