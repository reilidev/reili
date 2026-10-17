//! Lease: who runs the cycle, and for how long. Acquiring decides between starting a new cycle,
//! resuming an interrupted one, and giving up on it; renewing keeps the lease alive while the agent
//! runs.

use chrono::{DateTime, Duration, Utc};
use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{ActiveCycle, CyclePhase, CycleResult, ObjectiveLedgerState, StoredLedger};

use super::{CycleStart, ObjectiveCycleRunner};
use crate::string_log_meta;

pub(super) enum Acquisition {
    Run(ActiveCycle),
    Busy(String),
    Finished(CycleResult),
}

pub(super) struct AcquireInput<'a> {
    pub(super) objective: &'a ObjectiveDefinition,
    pub(super) start: &'a CycleStart,
    pub(super) instance_id: &'a str,
    pub(super) run_id: &'a str,
    pub(super) now: DateTime<Utc>,
    pub(super) lease: Duration,
}

impl ObjectiveCycleRunner {
    pub(super) async fn renew_lease(&self, ledger: &mut StoredLedger) {
        let expires = self.deps.clock.now() + self.deps.lease_duration;
        let instance_id = self.deps.instance_id.clone();
        match self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                if let Some(cycle) = state.active_cycle.as_mut()
                    && cycle.lease_holder == instance_id
                {
                    cycle.lease_expires_at = expires;
                }
            })
            .await
        {
            Ok((saved, ())) => *ledger = saved,
            Err(error) => self.deps.logger.warn(
                "objective_cycle_lease_renewal_failed",
                string_log_meta([
                    ("objectiveId", ledger.state.objective_id.to_string()),
                    ("error", error.to_string()),
                ]),
            ),
        }
    }
}

/// Decides, inside a ledger mutation, whether this instance runs a cycle now.
pub(super) fn acquire(state: &mut ObjectiveLedgerState, input: &AcquireInput<'_>) -> Acquisition {
    let acquisition = match state.active_cycle.clone() {
        Some(active) => resume_interrupted(state, active, input),
        None => start_new(state, input),
    };
    state.updated_at = input.now;
    acquisition
}

fn resume_interrupted(
    state: &mut ObjectiveLedgerState,
    mut active: ActiveCycle,
    input: &AcquireInput<'_>,
) -> Acquisition {
    if active.lease_is_live(input.now) {
        return Acquisition::Busy(format!("cycle {} is already running", active.run_id));
    }
    if active.attempt >= input.objective.budget.max_cycle_attempts {
        let reason = format!("interrupted {} time(s)", active.attempt);
        state.record_cycle(active.to_record(
            CycleResult::Abandoned {
                reason: format!("{reason} in phase {:?}", active.phase),
            },
            input.now,
            0,
        ));
        state.complete_slot(active.scheduled_for);
        state.active_cycle = None;
        return Acquisition::Finished(CycleResult::Abandoned { reason });
    }
    active.attempt += 1;
    active.lease_holder = input.instance_id.to_string();
    active.lease_expires_at = input.now + input.lease;
    state.active_cycle = Some(active.clone());
    Acquisition::Run(active)
}

fn start_new(state: &mut ObjectiveLedgerState, input: &AcquireInput<'_>) -> Acquisition {
    let CycleStart::New { scheduled_for } = input.start else {
        return Acquisition::Busy("no interrupted cycle to resume".to_string());
    };
    let active = ActiveCycle {
        run_id: input.run_id.to_string(),
        scheduled_for: *scheduled_for,
        started_at: input.now,
        attempt: 1,
        phase: CyclePhase::Observing,
        lease_holder: input.instance_id.to_string(),
        lease_expires_at: input.now + input.lease,
        pending_notifications: Vec::new(),
        pending_knowledge: Vec::new(),
        summary: String::new(),
        no_new_findings: false,
        llm_tokens: 0,
    };
    let daily_limit = input.objective.budget.max_llm_tokens_per_day;
    if state.tokens_used_on(input.now.date_naive()) >= daily_limit {
        let reason = format!("daily LLM token budget ({daily_limit}) exhausted");
        let mut record = active.to_record(
            CycleResult::Skipped {
                reason: reason.clone(),
            },
            input.now,
            0,
        );
        record.attempts = 0;
        state.record_cycle(record);
        state.complete_slot(*scheduled_for);
        return Acquisition::Finished(CycleResult::Skipped { reason });
    }
    state.active_cycle = Some(active.clone());
    Acquisition::Run(active)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use reili_core::work::{CycleResult, ObjectiveLedgerState};

    use super::{AcquireInput, Acquisition, acquire};
    use crate::objective::cycle::CycleStart;
    use crate::objective::testing::at;
    use crate::objective::tests_support::sample_objective;

    fn input<'a>(
        objective: &'a reili_core::objective::ObjectiveDefinition,
        start: &'a CycleStart,
        minutes: i64,
    ) -> AcquireInput<'a> {
        AcquireInput {
            objective,
            start,
            instance_id: "instance-a",
            run_id: "run-new",
            now: at("2026-10-15T00:00:00Z") + Duration::minutes(minutes),
            lease: Duration::minutes(15),
        }
    }

    #[test]
    fn starts_a_new_cycle_with_a_lease() {
        let objective = sample_objective("checkout");
        let mut state = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        let start = CycleStart::New {
            scheduled_for: Some(at("2026-10-15T00:00:00Z")),
        };

        let acquisition = acquire(&mut state, &input(&objective, &start, 0));

        let Acquisition::Run(active) = acquisition else {
            panic!("expected to run");
        };
        assert_eq!(active.attempt, 1);
        assert_eq!(active.lease_expires_at, at("2026-10-15T00:15:00Z"));
        assert_eq!(state.active_cycle, Some(active));
    }

    #[test]
    fn leaves_a_live_lease_alone_and_resumes_an_expired_one() {
        let objective = sample_objective("checkout");
        let mut state = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        let start = CycleStart::New {
            scheduled_for: None,
        };
        acquire(&mut state, &input(&objective, &start, 0));

        let busy = acquire(&mut state, &input(&objective, &CycleStart::Resume, 5));
        let resumed = acquire(&mut state, &input(&objective, &CycleStart::Resume, 20));

        assert!(matches!(busy, Acquisition::Busy(_)));
        let Acquisition::Run(active) = resumed else {
            panic!("expected to resume");
        };
        assert_eq!(active.run_id, "run-new");
        assert_eq!(active.attempt, 2);
    }

    #[test]
    fn abandons_a_cycle_after_too_many_interruptions() {
        let objective = sample_objective("checkout");
        let mut state = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        let slot = at("2026-10-15T00:00:00Z");
        let start = CycleStart::New {
            scheduled_for: Some(slot),
        };
        acquire(&mut state, &input(&objective, &start, 0));
        if let Some(active) = state.active_cycle.as_mut() {
            active.attempt = objective.budget.max_cycle_attempts;
        }

        let acquisition = acquire(&mut state, &input(&objective, &CycleStart::Resume, 20));

        assert!(matches!(
            acquisition,
            Acquisition::Finished(CycleResult::Abandoned { .. })
        ));
        assert!(state.active_cycle.is_none());
        assert_eq!(state.last_scheduled_for, Some(slot));
    }

    #[test]
    fn skips_a_new_cycle_when_the_daily_token_budget_is_spent() {
        let objective = sample_objective("checkout");
        let mut state = ObjectiveLedgerState::new(&objective, at("2026-10-15T00:00:00Z"));
        state
            .usage_for(at("2026-10-15T00:00:00Z").date_naive())
            .llm_tokens = objective.budget.max_llm_tokens_per_day;
        let start = CycleStart::New {
            scheduled_for: Some(at("2026-10-15T00:00:00Z")),
        };

        let acquisition = acquire(&mut state, &input(&objective, &start, 0));

        assert!(matches!(
            acquisition,
            Acquisition::Finished(CycleResult::Skipped { .. })
        ));
        assert!(state.active_cycle.is_none());
        assert_eq!(state.cycles.len(), 1);
        assert_eq!(state.cycles[0].attempts, 0);
    }
}
