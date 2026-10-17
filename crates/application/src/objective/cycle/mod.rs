//! One objective cycle: Observe → Plan/Investigate (agent) → Decide → Report → Learn.
//!
//! Durability: every phase boundary is checkpointed in the objective ledger record. A cycle
//! holds a time-limited lease that is renewed while the agent runs; if the process dies, the lease
//! expires and the next scheduler tick resumes the cycle. Phases after Decide never re-run the
//! agent, so a crash during reporting does not repeat (or pay for) the investigation. Work State
//! backends offer no transactions, so these are best-effort, at-least-once semantics: a crash
//! between a work item write and the ledger checkpoint re-runs the investigation, whose results
//! merge into the same work items by stable key instead of creating duplicates.
//!
//! One file per phase: `lease` (acquire and renew), `observe`, `investigate`, `apply` (Decide),
//! `notify` (Report), `learn`, and `finalize`.

mod apply;
mod finalize;
mod investigate;
mod learn;
mod lease;
mod notify;
mod observe;

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reili_core::error::PortError;
use reili_core::investigation::InvestigationRunnerPort;
use reili_core::messaging::slack::ObjectiveReportPort;
use reili_core::objective::ObjectiveDefinition;
use reili_core::task::{TaskCancellation, TaskResources};
use reili_core::work::{ActiveCycle, CyclePhase, CycleResult, StoredLedger, WorkStateError};
use thiserror::Error;
use uuid::Uuid;

use self::investigate::InvestigationResult;
use self::lease::{AcquireInput, Acquisition, acquire};
use self::notify::ReportError;
use super::clock::Clock;
use super::store::WorkStateStore;
use crate::{TaskLogger, string_log_meta};

#[derive(Clone)]
pub struct ObjectiveCycleDeps {
    pub store: WorkStateStore,
    pub investigation_runner: Arc<dyn InvestigationRunnerPort>,
    pub report_port: Arc<dyn ObjectiveReportPort>,
    pub task_resources: TaskResources,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
    /// Identifies this process as lease holder.
    pub instance_id: String,
    pub lease_duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleStart {
    New {
        scheduled_for: Option<DateTime<Utc>>,
    },
    /// Continue an interrupted cycle whose lease expired.
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleRunOutcome {
    Finished {
        run_id: String,
        result: CycleResult,
    },
    Skipped {
        reason: String,
    },
    /// Stopped without finalizing; the lease expires and a later tick resumes.
    Interrupted {
        run_id: String,
        reason: String,
    },
}

#[derive(Debug, Error)]
pub enum ObjectiveCycleError {
    #[error(transparent)]
    WorkState(#[from] WorkStateError),
    #[error("objective `{objective}` cannot run: {reason}")]
    NotRunnable { objective: String, reason: String },
}

pub struct ObjectiveCycleRunner {
    deps: ObjectiveCycleDeps,
}

impl ObjectiveCycleRunner {
    pub fn new(deps: ObjectiveCycleDeps) -> Self {
        Self { deps }
    }

    pub async fn run(
        &self,
        objective: &ObjectiveDefinition,
        start: CycleStart,
        cancellation: TaskCancellation,
    ) -> Result<CycleRunOutcome, ObjectiveCycleError> {
        if !objective.enabled {
            return Err(ObjectiveCycleError::NotRunnable {
                objective: objective.id.to_string(),
                reason: "objective is disabled in config".to_string(),
            });
        }
        let now = self.deps.clock.now();
        let ledger = self.deps.store.ensure_ledger(objective, now).await?;

        if let Some(active) = &ledger.state.active_cycle
            && active.lease_is_live(now)
        {
            return Ok(CycleRunOutcome::Skipped {
                reason: format!(
                    "cycle {} is already running (lease held by {} until {})",
                    active.run_id,
                    active.lease_holder,
                    active.lease_expires_at.to_rfc3339()
                ),
            });
        }
        if matches!(start, CycleStart::Resume) && ledger.state.active_cycle.is_none() {
            return Ok(CycleRunOutcome::Skipped {
                reason: "no interrupted cycle to resume".to_string(),
            });
        }

        let candidate_run_id = Uuid::new_v4().simple().to_string();
        let instance_id = self.deps.instance_id.clone();
        let lease = self.deps.lease_duration;
        let (ledger, acquisition) = self
            .deps
            .store
            .mutate_ledger(&ledger, |state| {
                acquire(
                    state,
                    &AcquireInput {
                        objective,
                        start: &start,
                        instance_id: &instance_id,
                        run_id: &candidate_run_id,
                        now,
                        lease,
                    },
                )
            })
            .await?;
        let active = match acquisition {
            Acquisition::Run(active) => active,
            Acquisition::Busy(reason) => return Ok(CycleRunOutcome::Skipped { reason }),
            Acquisition::Finished(result) => {
                self.notify_terminal_result(objective, &ledger, &result)
                    .await;
                return Ok(CycleRunOutcome::Finished {
                    run_id: candidate_run_id,
                    result,
                });
            }
        };

        // Read back to detect a concurrent instance that wrote its own lease in between.
        let confirmed = self.deps.store.reload_ledger(&objective.id).await?;
        let still_ours = confirmed.state.active_cycle.as_ref().is_some_and(|cycle| {
            cycle.run_id == active.run_id && cycle.lease_holder == self.deps.instance_id
        });
        if !still_ours {
            return Ok(CycleRunOutcome::Skipped {
                reason: "lost the cycle lease to another instance".to_string(),
            });
        }

        self.log("objective_cycle_started", objective, &active, None);
        self.continue_cycle(objective, confirmed, active, cancellation)
            .await
    }

    async fn continue_cycle(
        &self,
        objective: &ObjectiveDefinition,
        mut ledger: StoredLedger,
        mut active: ActiveCycle,
        cancellation: TaskCancellation,
    ) -> Result<CycleRunOutcome, ObjectiveCycleError> {
        let run_id = active.run_id.clone();
        if !active.phase.investigation_persisted() {
            let observed = self.observe(objective, &ledger).await?;
            let now = self.deps.clock.now();
            let lease = self.deps.lease_duration;
            let (saved, ()) = self
                .deps
                .store
                .mutate_ledger(&ledger, |state| {
                    if let Some(cycle) = state.active_cycle.as_mut() {
                        cycle.phase = CyclePhase::Investigating;
                        cycle.lease_expires_at = now + lease;
                    }
                    state.updated_at = now;
                })
                .await?;
            ledger = saved;

            let investigation = self
                .investigate(objective, &mut ledger, &active, &observed, &cancellation)
                .await?;
            match investigation {
                InvestigationResult::Interrupted { tokens } => {
                    self.record_usage(&ledger, tokens).await?;
                    return Ok(CycleRunOutcome::Interrupted {
                        run_id,
                        reason: "shutdown requested during investigation".to_string(),
                    });
                }
                InvestigationResult::Ended { result, tokens } => {
                    let result = self
                        .finalize(objective, &ledger, &active, result, tokens)
                        .await?;
                    return Ok(CycleRunOutcome::Finished { run_id, result });
                }
                InvestigationResult::Report { report, tokens } => {
                    ledger = self
                        .decide(objective, &ledger, &active, &observed, &report, tokens)
                        .await?;
                }
            }
            active = ledger
                .state
                .active_cycle
                .clone()
                .ok_or_else(|| missing_cycle(objective))?;
        }

        if active.phase == CyclePhase::Reporting {
            match self.report(objective, &ledger, &active).await {
                Ok(saved) => ledger = saved,
                Err(ReportError::WorkState(error)) => return Err(error.into()),
                Err(ReportError::Slack(error)) => {
                    self.log(
                        "objective_cycle_report_failed",
                        objective,
                        &active,
                        Some(&error.message),
                    );
                    return Ok(CycleRunOutcome::Interrupted {
                        run_id,
                        reason: format!("Slack report failed; will retry: {}", error.message),
                    });
                }
            }
            active = ledger
                .state
                .active_cycle
                .clone()
                .ok_or_else(|| missing_cycle(objective))?;
        }

        if active.phase == CyclePhase::Learning {
            ledger = self.learn(objective, &ledger, &active).await?;
        }

        let result = if active.no_new_findings {
            CycleResult::NoNewFindings
        } else {
            CycleResult::Completed
        };
        let result = self
            .finalize(objective, &ledger, &active, result, 0)
            .await?;
        Ok(CycleRunOutcome::Finished { run_id, result })
    }

    fn log(
        &self,
        event: &'static str,
        objective: &ObjectiveDefinition,
        active: &ActiveCycle,
        detail: Option<&str>,
    ) {
        let mut fields = string_log_meta([
            ("objectiveId", objective.id.to_string()),
            ("runId", active.run_id.clone()),
            ("attempt", active.attempt.to_string()),
            ("phase", format!("{:?}", active.phase)),
        ]);
        if let Some(detail) = detail {
            fields.insert("detail".to_string(), detail.to_string().into());
        }
        self.deps.logger.info(event, fields);
    }
}

fn missing_cycle(objective: &ObjectiveDefinition) -> ObjectiveCycleError {
    ObjectiveCycleError::WorkState(WorkStateError::Other(PortError::new(format!(
        "active cycle for `{}` disappeared from the ledger",
        objective.id
    ))))
}
