//! Observe: read the objective's items from the tracker, the discussion on Reili's own items, and
//! the Canvas memory.

use chrono::Duration;
use reili_core::messaging::slack::{ListSlackCanvasMemoriesInput, SlackCanvasMemoryRecord};
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{WorkItem, WorkStateError};

use super::super::brief::WorkThread;
use super::ObjectiveCycleRunner;

const MAX_MEMORIES_IN_BRIEF: u32 = 15;
const MAX_THREADS: usize = 15;
const CLOSED_ITEMS_LOOKBACK_DAYS: i64 = 30;

pub(super) struct Observed {
    pub(super) items: Vec<WorkItem>,
    pub(super) threads: Vec<WorkThread>,
    pub(super) memories: Vec<SlackCanvasMemoryRecord>,
    pub(super) notes: Vec<String>,
}

impl ObjectiveCycleRunner {
    pub(super) async fn observe(
        &self,
        objective: &ObjectiveDefinition,
    ) -> Result<Observed, WorkStateError> {
        let now = self.deps.clock.now();
        let items = self
            .deps
            .work_state
            .list_work_items(
                &objective.id,
                now - Duration::days(CLOSED_ITEMS_LOOKBACK_DAYS),
            )
            .await?;
        let mut notes = Vec::new();

        let mut own: Vec<&WorkItem> = items.iter().filter(|item| item.is_reili_work()).collect();
        own.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        if own.len() > MAX_THREADS {
            notes.push(format!(
                "{} items are assigned to you; only the {MAX_THREADS} most recently updated are shown",
                own.len()
            ));
        }
        let mut threads = Vec::new();
        for item in own.into_iter().take(MAX_THREADS) {
            threads.push(WorkThread {
                item: item.clone(),
                comments: self.deps.work_state.list_comments(&item.record).await?,
            });
        }

        let memories = match &self.deps.task_resources.canvas_memory_port {
            Some(port) => match port
                .list_channel_memories(ListSlackCanvasMemoriesInput {
                    channel_id: objective.report_channel_id.clone(),
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

        Ok(Observed {
            items,
            threads,
            memories,
            notes,
        })
    }
}
