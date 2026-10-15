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
    CycleTrigger, GoalLedgerState, GoalLifecycle, HumanCommand, StoredLedger, WorkStateComment,
    WorkStateCommentKind, WorkStateError, parse_human_comment,
};
use thiserror::Error;
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
    pub feedback_poll_interval: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunRequestAck {
    Queued,
    AlreadyQueued,
}

#[derive(Debug, Error)]
pub enum RunRequestError {
    #[error("unknown goal `{0}`")]
    UnknownGoal(String),
    #[error("goal `{0}` is disabled in config")]
    Disabled(String),
    #[error("goal `{goal}` was triggered by an alert less than {minutes} minutes ago")]
    RateLimited { goal: String, minutes: u32 },
    #[error(transparent)]
    WorkState(#[from] WorkStateError),
}

pub struct GoalScheduler {
    deps: GoalSchedulerDeps,
    running: Mutex<BTreeSet<GoalId>>,
    semaphore: Arc<Semaphore>,
    shutdown: TaskCancellation,
    started: AtomicBool,
    last_sync: Mutex<Option<DateTime<Utc>>>,
    last_feedback_poll: Mutex<Option<DateTime<Utc>>>,
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
            last_feedback_poll: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn goals(&self) -> &[GoalDefinition] {
        &self.deps.goals
    }

    #[must_use]
    pub fn store(&self) -> &WorkStateStore {
        &self.deps.store
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
        if let Err(error) = self.poll_issue_commands_if_due(now).await {
            self.warn("goal_feedback_poll_failed", None, &error.to_string());
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

    async fn poll_issue_commands_if_due(&self, now: DateTime<Utc>) -> Result<(), RunRequestError> {
        let since = {
            let mut last = self.last_feedback_poll.lock().await;
            match *last {
                Some(previous) if now < previous + self.deps.feedback_poll_interval => {
                    return Ok(());
                }
                previous => {
                    *last = Some(now);
                    previous
                }
            }
        };
        let Some(since) = since else {
            return Ok(());
        };
        for goal in self.deps.goals.iter().filter(|goal| goal.enabled) {
            let items = self
                .deps
                .store
                .repository()
                .list_work_items(&goal.id)
                .await?;
            for item in items
                .iter()
                .filter(|item| item.observed_status.is_open() && item.remote_updated_at > since)
            {
                let comments = self
                    .deps
                    .store
                    .repository()
                    .list_human_comments(&item.issue, item.state.last_processed_comment_id)
                    .await?;
                let rerun_by = comments.iter().find(|comment| {
                    comment.author_trusted
                        && parse_human_comment(&comment.body)
                            .commands
                            .contains(&HumanCommand::Rerun)
                });
                if let Some(comment) = rerun_by {
                    self.request_run(
                        goal.id.as_str(),
                        CycleTrigger::IssueCommand {
                            issue_number: item.issue.number,
                            author: comment.author.clone(),
                        },
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// Durably requests an out-of-schedule cycle (Slack request, alert, or issue command). The
    /// request is stored in the ledger, so it survives restarts; the next tick starts it.
    pub async fn request_run(
        &self,
        goal_id: &str,
        trigger: CycleTrigger,
    ) -> Result<RunRequestAck, RunRequestError> {
        let goal = self
            .deps
            .goals
            .iter()
            .find(|goal| goal.id.as_str() == goal_id)
            .ok_or_else(|| RunRequestError::UnknownGoal(goal_id.to_string()))?;
        if !goal.enabled {
            return Err(RunRequestError::Disabled(goal_id.to_string()));
        }
        let now = self.deps.clock.now();
        let ledger = self.deps.store.ensure_ledger(goal, now).await?;
        if ledger.state.requested_trigger.is_some() {
            return Ok(RunRequestAck::AlreadyQueued);
        }
        if matches!(trigger, CycleTrigger::Alert { .. })
            && let Some(alert) = &goal.alert_trigger
            && ledger.state.last_alert_trigger_at.is_some_and(|last| {
                now < last + Duration::minutes(i64::from(alert.min_interval_minutes))
            })
        {
            return Err(RunRequestError::RateLimited {
                goal: goal_id.to_string(),
                minutes: alert.min_interval_minutes,
            });
        }
        let (_, ack) = self
            .deps
            .store
            .mutate_ledger(&ledger, |state| {
                if state.requested_trigger.is_some() {
                    return RunRequestAck::AlreadyQueued;
                }
                if matches!(trigger, CycleTrigger::Alert { .. }) {
                    state.last_alert_trigger_at = Some(now);
                }
                state.requested_trigger = Some(trigger.clone());
                state.updated_at = now;
                RunRequestAck::Queued
            })
            .await?;
        Ok(ack)
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
    if let Some(trigger) = &ledger.requested_trigger {
        return Some(CycleStart::New {
            trigger: trigger.clone(),
            scheduled_for: None,
        });
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
        trigger: CycleTrigger::Schedule,
        scheduled_for: Some(slot),
    })
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use reili_core::work::{
        ActiveCycle, CyclePhase, CycleRecord, CycleResult, CycleTrigger, GoalLedgerState,
    };

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
                trigger: CycleTrigger::Schedule,
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
                trigger: CycleTrigger::Schedule,
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
            trigger: CycleTrigger::Schedule,
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
    fn requested_runs_win_and_backoff_delays_retries() {
        let goal = sample_goal("checkout");
        let mut ledger = GoalLedgerState::new(&goal, at("2026-10-15T00:00:00Z"));
        ledger.last_scheduled_for = Some(at("2026-10-15T03:00:00Z"));
        ledger.requested_trigger = Some(CycleTrigger::SlackRequest {
            user_id: "U1".to_string(),
        });
        assert!(matches!(
            decide_due(&goal, &ledger, at("2026-10-15T04:00:00Z")),
            Some(CycleStart::New {
                scheduled_for: None,
                ..
            })
        ));

        ledger.record_cycle(CycleRecord {
            run_id: "r".to_string(),
            trigger: CycleTrigger::Schedule,
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
