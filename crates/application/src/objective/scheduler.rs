use std::collections::BTreeSet;
use std::sync::{Arc, PoisonError};

use chrono::{DateTime, Utc};
use reili_core::objective::{ObjectiveDefinition, ObjectiveId};
use reili_core::task::TaskCancellation;
use reili_core::work::WorkStateError;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::clock::Clock;
use super::cycle::{CycleOutcome, ObjectiveCycleRunner};
use crate::{TaskLogger, string_log_meta};

const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

pub struct ObjectiveSchedulerDeps {
    pub objectives: Vec<ObjectiveDefinition>,
    pub cycle_runner: Arc<ObjectiveCycleRunner>,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
}

pub struct ObjectiveScheduler {
    deps: ObjectiveSchedulerDeps,
    running: std::sync::Mutex<BTreeSet<ObjectiveId>>,
    shutdown: TaskCancellation,
    last_checked: Mutex<DateTime<Utc>>,
}

impl ObjectiveScheduler {
    pub fn new(deps: ObjectiveSchedulerDeps) -> Arc<Self> {
        let now = deps.clock.now();
        Arc::new(Self {
            deps,
            running: std::sync::Mutex::new(BTreeSet::new()),
            shutdown: TaskCancellation::new(),
            last_checked: Mutex::new(now),
        })
    }

    #[must_use]
    pub fn objectives(&self) -> &[ObjectiveDefinition] {
        &self.deps.objectives
    }

    pub fn start(self: &Arc<Self>) {
        if self.deps.objectives.is_empty() {
            return;
        }
        let scheduler = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if scheduler.shutdown.is_cancelled() {
                    break;
                }
                let _ = scheduler.tick().await;
                tokio::select! {
                    () = tokio::time::sleep(TICK_INTERVAL) => {}
                    () = scheduler.shutdown.wait_for_cancellation() => break,
                }
            }
        });
    }

    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// Starts the cycles whose scheduled time passed since the previous tick.
    pub async fn tick(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let now = self.deps.clock.now();
        let since = std::mem::replace(&mut *self.last_checked.lock().await, now);

        let mut handles = Vec::new();
        for objective in &self.deps.objectives {
            if due_slot(objective, since, now).is_none() {
                continue;
            }
            let Some(running) = self.mark_running(&objective.id) else {
                self.log(
                    "objective_cycle_skipped",
                    &objective.id,
                    "the previous cycle is still running",
                );
                continue;
            };
            let scheduler = Arc::clone(self);
            let objective = objective.clone();
            handles.push(tokio::spawn(async move {
                let _running = running;
                let cycle = {
                    let scheduler = Arc::clone(&scheduler);
                    let objective = objective.clone();
                    tokio::spawn(async move {
                        scheduler
                            .deps
                            .cycle_runner
                            .run(&objective, scheduler.shutdown.clone())
                            .await
                    })
                };
                match cycle.await {
                    Ok(outcome) => scheduler.log_outcome(&objective.id, &outcome),
                    Err(error) => scheduler.log(
                        "objective_cycle_failed",
                        &objective.id,
                        &format!("the cycle stopped abnormally: {error}"),
                    ),
                }
            }));
        }
        handles
    }

    /// `None` when a cycle of the objective is already running.
    fn mark_running(self: &Arc<Self>, objective_id: &ObjectiveId) -> Option<RunningGuard> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(objective_id.clone())
            .then(|| RunningGuard {
                scheduler: Arc::clone(self),
                objective_id: objective_id.clone(),
            })
    }

    fn log_outcome(
        &self,
        objective_id: &ObjectiveId,
        outcome: &Result<CycleOutcome, WorkStateError>,
    ) {
        let (event, detail) = match outcome {
            Ok(CycleOutcome::Completed {
                run_id,
                created,
                updated,
                reported,
            }) => (
                "objective_cycle_finished",
                format!("{run_id}: {created} created, {updated} updated, reported={reported}"),
            ),
            Ok(CycleOutcome::NothingNew { run_id }) => {
                ("objective_cycle_finished", format!("{run_id}: nothing new"))
            }
            Ok(CycleOutcome::Ended { run_id, reason }) => {
                ("objective_cycle_ended", format!("{run_id}: {reason}"))
            }
            Ok(CycleOutcome::Cancelled { run_id }) => ("objective_cycle_cancelled", run_id.clone()),
            Err(error) => ("objective_cycle_failed", error.to_string()),
        };
        self.log(event, objective_id, &detail);
    }

    fn log(&self, event: &'static str, objective_id: &ObjectiveId, detail: &str) {
        self.deps.logger.info(
            event,
            string_log_meta([
                ("objectiveId", objective_id.to_string()),
                ("detail", detail.to_string()),
            ]),
        );
    }
}

/// Clears the running mark when the cycle ends, including when it panics.
struct RunningGuard {
    scheduler: Arc<ObjectiveScheduler>,
    objective_id: ObjectiveId,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.scheduler
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.objective_id);
    }
}

/// The latest scheduled time in `(since, now]`, if any; several missed times collapse into one.
#[must_use]
pub fn due_slot(
    objective: &ObjectiveDefinition,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if !objective.enabled {
        return None;
    }
    let next = objective.schedule.next_after(since)?;
    if next > now {
        return None;
    }
    objective.schedule.latest_at_or_before(now, next)
}

#[cfg(test)]
mod tests {
    use super::due_slot;
    use crate::objective::testing::at;
    use crate::objective::tests_support::sample_objective;

    #[test]
    fn runs_at_scheduled_times_only() {
        // every 6h in JST: 03:00Z, 09:00Z, 15:00Z, 21:00Z
        let objective = sample_objective("checkout");

        assert_eq!(
            due_slot(
                &objective,
                at("2026-10-15T02:59:00Z"),
                at("2026-10-15T03:00:30Z")
            ),
            Some(at("2026-10-15T03:00:00Z"))
        );
        assert_eq!(
            due_slot(
                &objective,
                at("2026-10-15T03:00:30Z"),
                at("2026-10-15T03:01:30Z")
            ),
            None
        );
    }

    #[test]
    fn collapses_times_missed_between_ticks() {
        let objective = sample_objective("checkout");

        assert_eq!(
            due_slot(
                &objective,
                at("2026-10-14T02:00:00Z"),
                at("2026-10-15T01:00:00Z")
            ),
            Some(at("2026-10-14T21:00:00Z"))
        );
    }

    #[test]
    fn disabled_objectives_never_run() {
        let mut objective = sample_objective("checkout");
        objective.enabled = false;

        assert_eq!(
            due_slot(
                &objective,
                at("2026-10-15T02:59:00Z"),
                at("2026-10-15T03:00:30Z")
            ),
            None
        );
    }
}
