//! The objective scheduler: runs each enabled objective's cycle at its scheduled times, like cron.
//! Nothing is persisted: a slot that passes while Reili is stopped (or while the objective's previous
//! cycle is still running) is skipped, and the next scheduled time picks the work up from the
//! tracker.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use reili_core::objective::{ObjectiveDefinition, ObjectiveId};
use reili_core::task::TaskCancellation;
use reili_core::work::WorkStateError;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

use super::clock::Clock;
use super::cycle::{CycleOutcome, ObjectiveCycleRunner};
use crate::{TaskLogger, string_log_meta};

pub struct ObjectiveSchedulerDeps {
    pub objectives: Vec<ObjectiveDefinition>,
    pub cycle_runner: Arc<ObjectiveCycleRunner>,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
    pub tick_interval: std::time::Duration,
    pub max_concurrent_cycles: usize,
}

pub struct ObjectiveScheduler {
    deps: ObjectiveSchedulerDeps,
    running: Mutex<BTreeSet<ObjectiveId>>,
    semaphore: Arc<Semaphore>,
    shutdown: TaskCancellation,
    started: AtomicBool,
    last_checked: Mutex<DateTime<Utc>>,
}

impl ObjectiveScheduler {
    pub fn new(deps: ObjectiveSchedulerDeps) -> Arc<Self> {
        let permits = deps.max_concurrent_cycles.max(1);
        let now = deps.clock.now();
        Arc::new(Self {
            deps,
            running: Mutex::new(BTreeSet::new()),
            semaphore: Arc::new(Semaphore::new(permits)),
            shutdown: TaskCancellation::new(),
            started: AtomicBool::new(false),
            last_checked: Mutex::new(now),
        })
    }

    #[must_use]
    pub fn objectives(&self) -> &[ObjectiveDefinition] {
        &self.deps.objectives
    }

    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) || self.deps.objectives.is_empty() {
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
                    () = tokio::time::sleep(scheduler.deps.tick_interval) => {}
                    () = scheduler.shutdown.wait_for_cancellation() => break,
                }
            }
        });
    }

    /// Stops scheduling and asks running cycles to stop.
    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// Starts the cycles whose scheduled time passed since the previous tick. Returns handles for
    /// the started cycles.
    pub async fn tick(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let now = self.deps.clock.now();
        let since = std::mem::replace(&mut *self.last_checked.lock().await, now);

        let mut handles = Vec::new();
        for objective in &self.deps.objectives {
            let Some(slot) = due_slot(objective, since, now) else {
                continue;
            };
            if self.running.lock().await.contains(&objective.id) {
                self.log(
                    "objective_cycle_skipped",
                    &objective.id,
                    "the previous cycle is still running",
                );
                continue;
            }
            let Ok(permit) = Arc::clone(&self.semaphore).try_acquire_owned() else {
                self.log(
                    "objective_cycle_skipped",
                    &objective.id,
                    "max_concurrent_cycles reached",
                );
                continue;
            };
            self.running.lock().await.insert(objective.id.clone());
            let scheduler = Arc::clone(self);
            let objective = objective.clone();
            handles.push(tokio::spawn(async move {
                let outcome = scheduler
                    .deps
                    .cycle_runner
                    .run(&objective, slot, scheduler.shutdown.clone())
                    .await;
                scheduler.log_outcome(&objective.id, &outcome);
                scheduler.running.lock().await.remove(&objective.id);
                drop(permit);
            }));
        }
        handles
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
