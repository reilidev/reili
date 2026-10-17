//! Work: run the agent on the brief within the objective's budget, deadline, and
//! shutdown signal, renewing the lease while it works.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Duration;
use reili_core::cycle_agent::{CycleAgentOutcome, RunBudget, RunCycleAgentInput, RunLimits};
use reili_core::error::PortError;
use reili_core::objective::ObjectiveDefinition;
use reili_core::task::{TaskCancellation, TaskProgressEventInput, TaskProgressEventPort};
use reili_core::work::{
    ActiveCycle, CycleReport, CycleResult, StoredLedger, sanitize::truncate_chars,
};

use super::super::brief::{BuildBriefInput, build_brief};
use super::observe::ObserveOutput;
use super::{ObjectiveCycleError, ObjectiveCycleRunner};

const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) enum WorkOutcome {
    Report {
        report: Box<CycleReport>,
        tokens: u64,
    },
    Ended {
        result: CycleResult,
        tokens: u64,
    },
    Interrupted {
        tokens: u64,
    },
}

impl ObjectiveCycleRunner {
    pub(super) async fn work(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &mut StoredLedger,
        active: &ActiveCycle,
        observed: &ObserveOutput,
        cancellation: &TaskCancellation,
    ) -> Result<WorkOutcome, ObjectiveCycleError> {
        let now = self.deps.clock.now();
        let max_duration = Duration::seconds(
            i64::try_from(objective.budget.max_cycle_duration_secs).unwrap_or(i64::MAX / 2),
        );
        let remaining_today = objective
            .budget
            .max_llm_tokens_per_day
            .saturating_sub(ledger.state.tokens_used_on(now.date_naive()));
        let budget = RunBudget::new(RunLimits {
            max_tool_calls: objective.budget.max_tool_calls_per_cycle,
            max_llm_tokens: objective
                .budget
                .max_llm_tokens_per_cycle
                .min(remaining_today)
                .max(1),
            max_sub_agents: objective.budget.max_sub_agents_per_cycle,
            max_parallel_tools: objective.budget.max_parallel_tools,
        });
        let brief = build_brief(&BuildBriefInput {
            objective,
            ledger: &ledger.state,
            ledger_human_notes: &ledger.human_notes,
            items: &observed.items,
            memories: &observed.memories,
            run_id: &active.run_id,
            now,
            deadline: now + max_duration,
            notes: &observed.notes,
        });
        let run_cancellation = TaskCancellation::new();
        let run_future = self.deps.cycle_agent.run(RunCycleAgentInput {
            brief,
            budget: budget.clone(),
            resources: self.deps.task_resources.clone(),
            cancellation: run_cancellation.clone(),
            on_progress_event: Arc::new(DiscardProgressEvents),
            logger: Arc::clone(&self.deps.logger),
        });
        tokio::pin!(run_future);
        let deadline = tokio::time::sleep(
            max_duration
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(1_800)),
        );
        tokio::pin!(deadline);
        let heartbeat_every = (self.deps.lease_duration / 3)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(60))
            .max(std::time::Duration::from_secs(1));
        let mut heartbeat = tokio::time::interval(heartbeat_every);
        heartbeat.tick().await;

        enum Ended {
            Finished(Result<CycleAgentOutcome, reili_core::error::AgentRunFailedError>),
            TimedOut,
            Shutdown,
        }
        let ended = loop {
            tokio::select! {
                result = &mut run_future => break Ended::Finished(result),
                () = &mut deadline => {
                    run_cancellation.cancel();
                    let _ = tokio::time::timeout(CANCEL_GRACE, &mut run_future).await;
                    break Ended::TimedOut;
                }
                () = cancellation.wait_for_cancellation() => {
                    run_cancellation.cancel();
                    let _ = tokio::time::timeout(CANCEL_GRACE, &mut run_future).await;
                    break Ended::Shutdown;
                }
                _ = heartbeat.tick() => {
                    self.renew_lease(ledger).await;
                }
            }
        };

        let tokens = budget.tokens_used();
        Ok(match ended {
            Ended::Shutdown => WorkOutcome::Interrupted { tokens },
            Ended::TimedOut => WorkOutcome::Ended {
                result: CycleResult::BudgetExhausted {
                    reason: format!(
                        "time limit of {}s reached before a report was submitted",
                        objective.budget.max_cycle_duration_secs
                    ),
                },
                tokens,
            },
            Ended::Finished(Err(error)) => WorkOutcome::Ended {
                result: CycleResult::Failed {
                    reason: truncate_chars(&error.cause_message, 300),
                },
                tokens: tokens.max(error.usage.total_tokens),
            },
            Ended::Finished(Ok(CycleAgentOutcome::Cancelled)) => {
                WorkOutcome::Interrupted { tokens }
            }
            Ended::Finished(Ok(CycleAgentOutcome::Completed(run))) => {
                let tokens = tokens.max(run.usage.total_tokens);
                match run.report {
                    Some(report) => WorkOutcome::Report {
                        report: Box::new(report),
                        tokens,
                    },
                    None if budget.tokens_exhausted()
                        || budget.tool_calls_used()
                            >= objective.budget.max_tool_calls_per_cycle =>
                    {
                        WorkOutcome::Ended {
                            result: CycleResult::BudgetExhausted {
                                reason: "budget exhausted before a report was submitted"
                                    .to_string(),
                            },
                            tokens,
                        }
                    }
                    None => WorkOutcome::Ended {
                        result: CycleResult::Failed {
                            reason: "the agent finished without submitting a structured report"
                                .to_string(),
                        },
                        tokens,
                    },
                }
            }
        })
    }
}

struct DiscardProgressEvents;

#[async_trait]
impl TaskProgressEventPort for DiscardProgressEvents {
    async fn publish(&self, _input: TaskProgressEventInput) -> Result<(), PortError> {
        Ok(())
    }
}
