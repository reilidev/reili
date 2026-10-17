//! Learn: save screened knowledge drafts to the Canvas memory, never the same fact twice.

use reili_core::messaging::slack::{AppendSlackCanvasMemoryInput, SlackCanvasMemoryVisibility};
use reili_core::objective::{ObjectiveDefinition, stable_fingerprint};
use reili_core::work::{ActiveCycle, StoredLedger};

use super::{ObjectiveCycleError, ObjectiveCycleRunner};

const MAX_FACTS_PER_CYCLE: usize = 5;

impl ObjectiveCycleRunner {
    pub(super) async fn learn(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
    ) -> Result<StoredLedger, ObjectiveCycleError> {
        let Some(canvas) = self
            .deps
            .task_resources
            .canvas_memory_port
            .as_ref()
            .filter(|_| objective.permissions.save_knowledge)
        else {
            return Ok(ledger.clone());
        };
        let mut learned = Vec::new();
        for draft in active.pending_knowledge.iter().take(MAX_FACTS_PER_CYCLE) {
            let fingerprint = stable_fingerprint(&draft.fact.to_lowercase());
            if ledger.state.has_learned(&fingerprint) {
                continue;
            }
            let result = canvas
                .append_memory(AppendSlackCanvasMemoryInput {
                    visibility: if draft.shared {
                        SlackCanvasMemoryVisibility::Shared
                    } else {
                        SlackCanvasMemoryVisibility::Channel
                    },
                    channel_id: objective.report.channel_id.clone(),
                    channel_name: None,
                    source_message_ts: String::new(),
                    source_url: Some(ledger.record.url.clone()),
                    fact: draft.fact.clone(),
                    evidence: draft.evidence.clone(),
                    scope: draft.scope.clone(),
                })
                .await;
            match result {
                Ok(()) => learned.push(fingerprint),
                Err(error) => self.log(
                    "objective_cycle_learn_failed",
                    objective,
                    active,
                    Some(&error.message),
                ),
            }
        }
        if learned.is_empty() {
            return Ok(ledger.clone());
        }
        let (saved, ()) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                for fingerprint in &learned {
                    if !state.has_learned(fingerprint) {
                        state.remember_learned(fingerprint.clone());
                    }
                }
            })
            .await?;
        Ok(saved)
    }
}
