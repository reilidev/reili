//! Finalize: record the cycle result in the ledger, release the lease, and give up on a slot after
//! too many failed attempts.

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{ActiveCycle, CycleResult, StoredLedger};

use super::{ObjectiveCycleError, ObjectiveCycleRunner};

impl ObjectiveCycleRunner {
    pub(super) async fn finalize(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
        result: CycleResult,
        extra_tokens: u64,
    ) -> Result<CycleResult, ObjectiveCycleError> {
        let now = self.deps.clock.now();
        let max_attempts = objective.budget.max_cycle_attempts as usize;
        let active = active.clone();
        let (saved, final_result) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                if extra_tokens > 0 {
                    state.usage_for(now.date_naive()).llm_tokens += extra_tokens;
                }
                let failed = matches!(result, CycleResult::Failed { .. });
                let prior_failures_for_slot = state
                    .cycles
                    .iter()
                    .rev()
                    .take_while(|record| {
                        matches!(record.result, CycleResult::Failed { .. })
                            && record.scheduled_for == active.scheduled_for
                    })
                    .count();
                let give_up = failed && prior_failures_for_slot + 1 >= max_attempts;
                let final_result = if give_up {
                    CycleResult::Abandoned {
                        reason: format!(
                            "{} (gave up after {} attempts)",
                            result.label(),
                            prior_failures_for_slot + 1
                        ),
                    }
                } else {
                    result.clone()
                };
                state.record_cycle(active.to_record(final_result.clone(), now, extra_tokens));
                state.active_cycle = None;
                if !failed || give_up {
                    state.complete_slot(active.scheduled_for);
                }
                state.updated_at = now;
                final_result
            })
            .await?;
        self.log(
            "objective_cycle_finished",
            objective,
            &active,
            Some(&final_result.label()),
        );
        self.notify_terminal_result(objective, &saved, &final_result)
            .await;
        Ok(final_result)
    }

    pub(super) async fn record_usage(
        &self,
        ledger: &StoredLedger,
        tokens: u64,
    ) -> Result<(), ObjectiveCycleError> {
        if tokens == 0 {
            return Ok(());
        }
        let now = self.deps.clock.now();
        self.deps
            .store
            .mutate_ledger(ledger, |state| {
                state.usage_for(now.date_naive()).llm_tokens += tokens;
                if let Some(cycle) = state.active_cycle.as_mut() {
                    cycle.llm_tokens += tokens;
                }
            })
            .await?;
        Ok(())
    }
}
