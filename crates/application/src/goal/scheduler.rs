//! The goal scheduler: on each tick it evaluates configured goals against their durable ledgers
//! and starts (or resumes) cycles that are due. All scheduling state lives in the ledgers, so a
//! restarted process continues where the previous one stopped; in-memory state is only a guard
//! against starting the same goal twice in one process.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, Utc};
use reili_core::goal::{GoalDefinition, GoalId, truncate_to_minute};
use reili_core::task::TaskCancellation;
use reili_core::work::{
    GoalLedgerState, GoalLifecycle, StoredLedger, WorkStateComment, WorkStateCommentKind,
    WorkStateError,
};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinHandle;

use super::clock::Clock;
use super::cycle::{CycleRunOutcome, CycleStart, GoalCycleError, GoalCycleRunner};
use super::store::WorkStateStore;
use crate::{TaskLogger, string_log_meta};

const LEDGER_SYNC_INTERVAL_MINUTES: i64 = 60;

pub struct GoalSchedulerDeps {
    pub goals: Vec<GoalDefinition>,
    pub cycle_runner: Arc<GoalCycleRunner>,
    pub store: WorkStateStore,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
    pub tick_interval: std::time::Duration,
    pub max_concurrent_cycles: usize,
}

pub struct GoalScheduler {
    deps: GoalSchedulerDeps,
    running: Mutex<BTreeSet<GoalId>>,
    semaphore: Arc<Semaphore>,
    shutdown: TaskCancellation,
    started: AtomicBool,
    last_sync: Mutex<Option<DateTime<Utc>>>,
}

impl GoalScheduler {
    pub fn new(deps: GoalSchedulerDeps) -> Arc<Self> {
        let permits = deps.max_concurrent_cycles.max(1);
        Arc::new(Self {
            deps,
            running: Mutex::new(BTreeSet::new()),
            semaphore: Arc::new(Semaphore::new(permits)),
            shutdown: TaskCancellation::new(),
            started: AtomicBool::new(false),
            last_sync: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn goals(&self) -> &[GoalDefinition] {
        &self.deps.goals
    }

    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) || self.deps.goals.is_empty() {
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

    /// Stops scheduling and asks running cycles to stop; interrupted cycles resume after restart.
    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// Evaluates every goal once and starts due cycles. Returns handles for the started cycles.
    pub async fn tick(self: &Arc<Self>) -> Vec<JoinHandle<()>> {
        let now = self.deps.clock.now();
        if let Err(error) = self.sync_if_due(now).await {
            self.warn("goal_ledger_sync_failed", None, &error.to_string());
            // Without readable ledgers nothing can be scheduled safely; try again next tick.
            return Vec::new();
        }

        let mut handles = Vec::new();
        for goal in self.deps.goals.iter().filter(|goal| goal.enabled) {
            if self.running.lock().await.contains(&goal.id) {
                continue;
            }
            let ledger = match self
                .deps
                .store
                .repository()
                .find_goal_ledger(&goal.id)
                .await
            {
                Ok(Some(ledger)) => ledger,
                Ok(None) => continue,
                Err(error) => {
                    self.warn(
                        "goal_ledger_read_failed",
                        Some(&goal.id),
                        &error.to_string(),
                    );
                    continue;
                }
            };
            let Some(start) = decide_due(goal, &ledger.state, now) else {
                continue;
            };
            let Ok(permit) = Arc::clone(&self.semaphore).try_acquire_owned() else {
                break;
            };
            self.running.lock().await.insert(goal.id.clone());
            let scheduler = Arc::clone(self);
            let goal = goal.clone();
            handles.push(tokio::spawn(async move {
                let outcome = scheduler
                    .deps
                    .cycle_runner
                    .run(&goal, start, scheduler.shutdown.clone())
                    .await;
                scheduler.log_outcome(&goal.id, &outcome);
                scheduler.running.lock().await.remove(&goal.id);
                drop(permit);
            }));
        }
        handles
    }

    async fn sync_if_due(&self, now: DateTime<Utc>) -> Result<(), WorkStateError> {
        let mut last_sync = self.last_sync.lock().await;
        if last_sync
            .is_some_and(|last| now < last + Duration::minutes(LEDGER_SYNC_INTERVAL_MINUTES))
        {
            return Ok(());
        }
        self.sync_ledgers(now).await?;
        *last_sync = Some(now);
        Ok(())
    }

    /// Aligns ledgers with config: creates missing ledgers, records definition changes, marks
    /// disabled goals, and retires ledgers whose goal was removed (their work items stay open for
    /// humans; Reili simply stops working on them).
    pub async fn sync_ledgers(&self, now: DateTime<Utc>) -> Result<(), WorkStateError> {
        for goal in &self.deps.goals {
            let ledger = self.deps.store.ensure_ledger(goal, now).await?;
            let fingerprint = goal.fingerprint();
            let lifecycle = if goal.enabled {
                GoalLifecycle::Active
            } else {
                GoalLifecycle::Disabled
            };
            if ledger.state.definition_fingerprint == fingerprint
                && ledger.state.lifecycle == lifecycle
            {
                continue;
            }
            let changed = ledger.state.definition_fingerprint != fingerprint;
            let previous_lifecycle = ledger.state.lifecycle;
            let objective = goal.objective.clone();
            let (saved, ()) = self
                .deps
                .store
                .mutate_ledger(&ledger, |state| {
                    if changed {
                        state.definition_fingerprint.clone_from(&fingerprint);
                        state.objective.clone_from(&objective);
                        state.definition_changes.push(now);
                        if state.definition_changes.len() > 20 {
                            state.definition_changes.remove(0);
                        }
                    }
                    state.lifecycle = lifecycle;
                    state.updated_at = now;
                })
                .await?;
            let mut notices = Vec::new();
            if changed {
                notices.push("Goal definition changed in config. Existing work items are kept; the next cycle re-checks whether each is still in scope.".to_string());
            }
            if previous_lifecycle != lifecycle {
                notices.push(format!(
                    "Goal lifecycle `{}` → `{}`.{}",
                    previous_lifecycle.as_str(),
                    lifecycle.as_str(),
                    if lifecycle == GoalLifecycle::Disabled {
                        " No cycles will run; open work items are left untouched."
                    } else {
                        ""
                    }
                ));
            }
            self.notice(&saved, notices).await?;
        }

        let configured: BTreeSet<&GoalId> = self.deps.goals.iter().map(|goal| &goal.id).collect();
        for ledger in self.deps.store.repository().list_goal_ledgers().await? {
            if configured.contains(&ledger.state.goal_id)
                || ledger.state.lifecycle == GoalLifecycle::Retired
            {
                continue;
            }
            let (saved, ()) = self
                .deps
                .store
                .mutate_ledger(&ledger, |state| {
                    state.lifecycle = GoalLifecycle::Retired;
                    state.updated_at = now;
                })
                .await?;
            self.notice(
                &saved,
                vec!["Goal removed from config: retired. Reili will not run cycles for it; its work items remain for humans to close or keep.".to_string()],
            )
            .await?;
        }
        Ok(())
    }

    async fn notice(
        &self,
        ledger: &StoredLedger,
        notices: Vec<String>,
    ) -> Result<(), WorkStateError> {
        if notices.is_empty() {
            return Ok(());
        }
        self.deps
            .store
            .repository()
            .append_comment(
                &ledger.issue,
                WorkStateComment {
                    kind: WorkStateCommentKind::Notice,
                    body: notices.join("\n"),
                },
            )
            .await
    }

    fn log_outcome(&self, goal_id: &GoalId, outcome: &Result<CycleRunOutcome, GoalCycleError>) {
        let (event, detail) = match outcome {
            Ok(CycleRunOutcome::Finished { run_id, result }) => (
                "goal_cycle_completed",
                format!("{run_id}: {}", result.label()),
            ),
            Ok(CycleRunOutcome::Skipped { reason }) => ("goal_cycle_skipped", reason.clone()),
            Ok(CycleRunOutcome::Interrupted { run_id, reason }) => {
                ("goal_cycle_interrupted", format!("{run_id}: {reason}"))
            }
            Err(error) => ("goal_cycle_aborted", error.to_string()),
        };
        self.deps.logger.info(
            event,
            string_log_meta([("goalId", goal_id.to_string()), ("detail", detail)]),
        );
    }

    fn warn(&self, event: &'static str, goal_id: Option<&GoalId>, error: &str) {
        self.deps.logger.warn(
            event,
            string_log_meta([
                (
                    "goalId",
                    goal_id.map_or_else(String::new, ToString::to_string),
                ),
                ("error", error.to_string()),
            ]),
        );
    }
}

/// Decides whether a goal has work to start now, from durable ledger state only.
#[must_use]
pub fn decide_due(
    goal: &GoalDefinition,
    ledger: &GoalLedgerState,
    now: DateTime<Utc>,
) -> Option<CycleStart> {
    if !goal.enabled || ledger.lifecycle == GoalLifecycle::Retired {
        return None;
    }
    if let Some(active) = &ledger.active_cycle {
        return (!active.lease_is_live(now)).then_some(CycleStart::Resume);
    }
    if ledger
        .failure_backoff_until()
        .is_some_and(|until| now < until)
    {
        return None;
    }
    let slot = match ledger.last_scheduled_for {
        // Baseline cycle for a new goal. Anchored to the ledger's creation so retries of a
        // failed baseline count against the same slot.
        None => Some(truncate_to_minute(ledger.created_at)),
        Some(last) => goal
            .schedule
            .next_after(last)
            .filter(|next| *next <= now)
            .and_then(|next| goal.schedule.latest_at_or_before(now, next)),
    }?;
    Some(CycleStart::New {
        scheduled_for: Some(slot),
    })
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use reili_core::work::{ActiveCycle, CyclePhase, CycleRecord, CycleResult, GoalLedgerState};

    use super::decide_due;
    use crate::goal::cycle::CycleStart;
    use crate::goal::testing::at;
    use crate::goal::tests_support::sample_goal;

    #[test]
    fn first_cycle_runs_immediately_as_baseline() {
        let goal = sample_goal("checkout");
        let ledger = GoalLedgerState::new(&goal, at("2026-10-15T01:00:00Z"));

        assert_eq!(
            decide_due(&goal, &ledger, at("2026-10-15T01:00:30Z")),
            Some(CycleStart::New {
                scheduled_for: Some(at("2026-10-15T01:00:00Z")),
            })
        );
    }

    #[test]
    fn coalesces_missed_slots_into_latest() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-14T00:00:00Z"));
        // every 6h in JST: 03:00Z, 09:00Z, 15:00Z, 21:00Z
        ledger.last_scheduled_for = Some(at("2026-10-14T03:00:00Z"));

        assert_eq!(
            decide_due(&goal, &ledger, at("2026-10-15T01:00:00Z")),
            Some(CycleStart::New {
                scheduled_for: Some(at("2026-10-14T21:00:00Z")),
            })
        );
        ledger.last_scheduled_for = Some(at("2026-10-14T21:00:00Z"));
        assert_eq!(decide_due(&goal, &ledger, at("2026-10-15T01:00:00Z")), None);
    }

    #[test]
    fn disabled_goals_never_run() {
        let mut goal = sample_goal("checkout");
        goal.enabled = false;
        let ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));

        assert_eq!(decide_due(&goal, &ledger, at("2026-10-15T01:00:00Z")), None);
    }

    #[test]
    fn resumes_only_after_lease_expiry() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        ledger.active_cycle = Some(ActiveCycle {
            run_id: "r1".to_string(),
            scheduled_for: None,
            started_at: at("2026-10-15T00:00:00Z"),
            attempt: 1,
            phase: CyclePhase::Investigating,
            lease_holder: "old".to_string(),
            lease_expires_at: at("2026-10-15T00:15:00Z"),
            pending_notifications: Vec::new(),
            pending_knowledge: Vec::new(),
            summary: String::new(),
            no_new_findings: false,
            llm_tokens: 0,
        });

        assert_eq!(decide_due(&goal, &ledger, at("2026-10-15T00:10:00Z")), None);
        assert_eq!(
            decide_due(&goal, &ledger, at("2026-10-15T00:16:00Z")),
            Some(CycleStart::Resume)
        );
    }

    #[test]
    fn backoff_delays_retries() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        assert!(decide_due(&goal, &ledger, at("2026-10-15T04:00:00Z")).is_some());

        ledger.record_cycle(CycleRecord {
            run_id: "r".to_string(),
            scheduled_for: None,
            started_at: at("2026-10-15T04:00:00Z"),
            finished_at: at("2026-10-15T04:00:00Z"),
            attempts: 1,
            result: CycleResult::Failed {
                reason: "x".to_string(),
            },
            llm_tokens: 0,
            summary: String::new(),
            touched_items: Vec::new(),
        });
        assert_eq!(
            decide_due(
                &goal,
                &ledger,
                at("2026-10-15T04:00:00Z") + Duration::minutes(2)
            ),
            None
        );
        assert!(
            decide_due(
                &goal,
                &ledger,
                at("2026-10-15T04:00:00Z") + Duration::minutes(6)
            )
            .is_some()
        );
    }
}
