//! One objective cycle: Observe → Work (agent) → Decide → Report → Learn. The agent chooses the
//! work that moves the objective forward (investigating, reviewing, following up, proposing); the
//! cycle does not depend on what kind of work it is.
//!
//! A cycle keeps no state of its own. Everything it needs is on the tracker (items labelled for the
//! objective, Reili's progress comments, the team's comments), so a cycle that dies halfway simply
//! leaves the comments it already wrote, and the next scheduled cycle starts from the tracker
//! again.
//!
//! One file per phase: `observe`, `work`, `apply` (Decide), `notify` (Report), and `learn`.

mod apply;
mod learn;
mod notify;
mod observe;
mod work;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use reili_core::cycle_agent::CycleAgentPort;
use reili_core::messaging::slack::ObjectiveReportPort;
use reili_core::objective::ObjectiveDefinition;
use reili_core::task::{TaskCancellation, TaskResources};
use reili_core::work::{WorkStateError, WorkStatePort};
use uuid::Uuid;

use self::work::WorkOutcome;
use super::clock::Clock;
use super::policy::apply_report_policy;
use crate::{TaskLogger, string_log_meta};

#[derive(Clone)]
pub struct ObjectiveCycleDeps {
    pub work_state: Arc<dyn WorkStatePort>,
    pub cycle_agent: Arc<dyn CycleAgentPort>,
    pub report_port: Arc<dyn ObjectiveReportPort>,
    pub task_resources: TaskResources,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleOutcome {
    Completed {
        run_id: String,
        created: usize,
        updated: usize,
        reported: bool,
    },
    NothingNew {
        run_id: String,
    },
    /// The agent failed or ran out of budget before submitting a report; nothing was written.
    Ended {
        run_id: String,
        reason: String,
    },
    Cancelled {
        run_id: String,
    },
}

pub struct ObjectiveCycleRunner {
    deps: ObjectiveCycleDeps,
}

impl ObjectiveCycleRunner {
    pub fn new(deps: ObjectiveCycleDeps) -> Self {
        Self { deps }
    }

    /// Runs the cycle for the scheduled `slot` (which also keys the Slack report, so a re-run of
    /// the same slot never posts twice).
    pub async fn run(
        &self,
        objective: &ObjectiveDefinition,
        slot: DateTime<Utc>,
        cancellation: TaskCancellation,
    ) -> Result<CycleOutcome, WorkStateError> {
        let run_id = Uuid::new_v4().simple().to_string();
        self.log("objective_cycle_started", objective, &run_id, None);
        let observed = self.observe(objective).await?;
        let (report, tokens) = match self
            .work(objective, &run_id, &observed, &cancellation)
            .await
        {
            WorkOutcome::Report { report, tokens } => (report, tokens),
            WorkOutcome::Ended { reason, tokens } => {
                self.log_tokens(objective, &run_id, tokens);
                return Ok(CycleOutcome::Ended { run_id, reason });
            }
            WorkOutcome::Cancelled { tokens } => {
                self.log_tokens(objective, &run_id, tokens);
                return Ok(CycleOutcome::Cancelled { run_id });
            }
        };
        self.log_tokens(objective, &run_id, tokens);

        let policy = apply_report_policy(&report);
        for rejected in &policy.rejected_knowledge {
            self.log(
                "objective_knowledge_rejected",
                objective,
                &run_id,
                Some(rejected),
            );
        }
        let activity = self
            .apply(objective, &run_id, &observed, &report, &policy.proposals)
            .await?;
        for note in &activity.skipped {
            self.log("objective_proposal_skipped", objective, &run_id, Some(note));
        }
        let reported = self
            .notify(objective, &run_id, slot, &report.summary, &activity)
            .await;
        if objective.permissions.save_knowledge {
            self.learn(objective, &run_id, &observed, &policy.knowledge)
                .await;
        }

        if activity.is_empty() {
            return Ok(CycleOutcome::NothingNew { run_id });
        }
        Ok(CycleOutcome::Completed {
            run_id,
            created: activity.created.len(),
            updated: activity.updated.len(),
            reported,
        })
    }

    fn log_tokens(&self, objective: &ObjectiveDefinition, run_id: &str, tokens: u64) {
        self.log(
            "objective_cycle_usage",
            objective,
            run_id,
            Some(&format!("{tokens} LLM tokens")),
        );
    }

    fn log(
        &self,
        event: &'static str,
        objective: &ObjectiveDefinition,
        run_id: &str,
        detail: Option<&str>,
    ) {
        let mut fields = string_log_meta([
            ("objectiveId", objective.id.to_string()),
            ("runId", run_id.to_string()),
        ]);
        if let Some(detail) = detail {
            fields.insert("detail".to_string(), detail.to_string().into());
        }
        self.deps.logger.info(event, fields);
    }
}
