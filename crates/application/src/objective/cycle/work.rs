//! Work: run the agent on the brief within the objective's budget, deadline, and shutdown signal.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Duration;
use reili_core::cycle_agent::{CycleAgentOutcome, RunBudget, RunCycleAgentInput, RunLimits};
use reili_core::error::{AgentRunFailedError, PortError};
use reili_core::objective::ObjectiveDefinition;
use reili_core::task::{TaskCancellation, TaskProgressEventInput, TaskProgressEventPort};
use reili_core::work::{CycleReport, sanitize::truncate_chars};

use super::super::brief::{BuildBriefInput, build_brief};
use super::ObjectiveCycleRunner;
use super::observe::Observed;

const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_SUB_AGENTS_PER_CYCLE: u32 = 12;
const MAX_PARALLEL_TOOLS: u32 = 4;

pub(super) enum WorkOutcome {
    Report {
        report: Box<CycleReport>,
        tokens: u64,
    },
    Ended {
        reason: String,
        tokens: u64,
    },
    Cancelled {
        tokens: u64,
    },
}

impl ObjectiveCycleRunner {
    pub(super) async fn work(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        observed: &Observed,
        cancellation: &TaskCancellation,
    ) -> WorkOutcome {
        let now = self.deps.clock.now();
        let max_duration = Duration::seconds(
            i64::try_from(objective.budget.max_cycle_duration_secs).unwrap_or(i64::MAX / 2),
        );
        let budget = RunBudget::new(RunLimits {
            max_tool_calls: objective.budget.max_tool_calls_per_cycle,
            max_llm_tokens: objective.budget.max_llm_tokens_per_cycle,
            max_sub_agents: MAX_SUB_AGENTS_PER_CYCLE,
            max_parallel_tools: MAX_PARALLEL_TOOLS,
        });
        let brief = build_brief(&BuildBriefInput {
            objective,
            threads: &observed.threads,
            items: &observed.items,
            memories: &observed.memories,
            run_id,
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

        enum Ended {
            Finished(Result<CycleAgentOutcome, AgentRunFailedError>),
            TimedOut,
            Shutdown,
        }
        let ended = tokio::select! {
            result = &mut run_future => Ended::Finished(result),
            () = &mut deadline => {
                run_cancellation.cancel();
                let _ = tokio::time::timeout(CANCEL_GRACE, &mut run_future).await;
                Ended::TimedOut
            }
            () = cancellation.wait_for_cancellation() => {
                run_cancellation.cancel();
                let _ = tokio::time::timeout(CANCEL_GRACE, &mut run_future).await;
                Ended::Shutdown
            }
        };

        let tokens = budget.tokens_used();
        match ended {
            Ended::Shutdown | Ended::Finished(Ok(CycleAgentOutcome::Cancelled)) => {
                WorkOutcome::Cancelled { tokens }
            }
            Ended::TimedOut => WorkOutcome::Ended {
                reason: format!(
                    "time limit of {}s reached before a report was submitted",
                    objective.budget.max_cycle_duration_secs
                ),
                tokens,
            },
            Ended::Finished(Err(error)) => WorkOutcome::Ended {
                reason: truncate_chars(&error.cause_message, 300),
                tokens: tokens.max(error.usage.total_tokens),
            },
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
                            reason: "budget exhausted before a report was submitted".to_string(),
                            tokens,
                        }
                    }
                    None => WorkOutcome::Ended {
                        reason: "the agent finished without submitting a structured report"
                            .to_string(),
                        tokens,
                    },
                }
            }
        }
    }
}

struct DiscardProgressEvents;

#[async_trait]
impl TaskProgressEventPort for DiscardProgressEvents {
    async fn publish(&self, _input: TaskProgressEventInput) -> Result<(), PortError> {
        Ok(())
    }
}
