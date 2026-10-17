//! One objective cycle: Observe → Plan/Investigate (agent) → Decide → Report → Learn.
//!
//! Durability: every phase boundary is checkpointed in the objective ledger record. A cycle
//! holds a time-limited lease that is renewed while the agent runs; if the process dies, the lease
//! expires and the next scheduler tick resumes the cycle. Phases after Decide never re-run the
//! agent, so a crash during reporting does not repeat (or pay for) the investigation. Work State
//! backends offer no transactions, so these are best-effort, at-least-once semantics: a crash between a
//! work item write and the ledger checkpoint re-runs the investigation, whose results merge into
//! the same work items by stable key instead of creating duplicates.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reili_core::error::PortError;
use reili_core::investigation::{
    InvestigationRunOutcome, InvestigationRunnerPort, RunBudget, RunInvestigationInput, RunLimits,
};
use reili_core::messaging::slack::{
    AppendSlackCanvasMemoryInput, ListSlackCanvasMemoriesInput, ObjectiveReportMessage,
    ObjectiveReportPort, SlackCanvasMemoryVisibility,
};
use reili_core::objective::{ObjectiveDefinition, stable_fingerprint};
use reili_core::task::{
    TaskCancellation, TaskProgressEventInput, TaskProgressEventPort, TaskResources,
};
use reili_core::work::{
    ActiveCycle, CyclePhase, CycleRecord, CycleResult, InvestigationReport, KnowledgeDraft,
    ObjectiveLedgerState, StoredLedger, StoredWorkItem, WorkStateComment, WorkStateCommentKind,
    WorkStateError, sanitize::truncate_chars,
};
use thiserror::Error;
use uuid::Uuid;

use super::brief::{BuildBriefInput, build_brief};
use super::clock::Clock;
use super::decide::{
    ApplyContext, PlanChangesInput, PlannedChange, apply_proposal, new_work_item_state,
    plan_changes,
};
use super::policy::apply_report_policy;
use super::reconcile::reconcile_human_signals;
use super::report::{
    ComposeReportInput, ReportDecision, compose_report, notification_digest, report_key,
};
use super::store::WorkStateStore;
use crate::{TaskLogger, string_log_meta};

const MAX_MEMORIES_IN_BRIEF: u32 = 15;
const MAX_FACTS_PER_CYCLE: usize = 5;
const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

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

enum Acquisition {
    Run(ActiveCycle),
    Busy(String),
    Finished(CycleResult),
}

struct ObserveOutput {
    items: Vec<StoredWorkItem>,
    memories: Vec<reili_core::messaging::slack::SlackCanvasMemoryRecord>,
    notes: Vec<String>,
}

enum InvestigationResult {
    Report {
        report: Box<InvestigationReport>,
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
                    objective,
                    &start,
                    &instance_id,
                    &candidate_run_id,
                    now,
                    lease,
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

    async fn observe(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
    ) -> Result<ObserveOutput, ObjectiveCycleError> {
        let now = self.deps.clock.now();
        let items = self
            .deps
            .store
            .repository()
            .list_work_items(&objective.id)
            .await?;
        let reconciled = reconcile_human_signals(&self.deps.store, items, now).await?;
        let mut notes = reconciled.notes;

        let items = reconciled.items;

        let memories = match &self.deps.task_resources.canvas_memory_port {
            Some(port) => match port
                .list_channel_memories(ListSlackCanvasMemoriesInput {
                    channel_id: objective.report.channel_id.clone(),
                    channel_name: None,
                    limit: MAX_MEMORIES_IN_BRIEF,
                })
                .await
            {
                Ok(memories) => memories,
                Err(error) => {
                    notes.push(format!("Canvas memory unavailable: {}", error.message));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        if ledger.state.definition_fingerprint != objective.fingerprint() {
            notes.push("objective definition fingerprint differs from the ledger".to_string());
        }

        Ok(ObserveOutput {
            items,
            memories,
            notes,
        })
    }

    async fn investigate(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &mut StoredLedger,
        active: &ActiveCycle,
        observed: &ObserveOutput,
        cancellation: &TaskCancellation,
    ) -> Result<InvestigationResult, ObjectiveCycleError> {
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
        let run_future = self.deps.investigation_runner.run(RunInvestigationInput {
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
            Finished(Result<InvestigationRunOutcome, reili_core::error::AgentRunFailedError>),
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
            Ended::Shutdown => InvestigationResult::Interrupted { tokens },
            Ended::TimedOut => InvestigationResult::Ended {
                result: CycleResult::BudgetExhausted {
                    reason: format!(
                        "time limit of {}s reached before a report was submitted",
                        objective.budget.max_cycle_duration_secs
                    ),
                },
                tokens,
            },
            Ended::Finished(Err(error)) => InvestigationResult::Ended {
                result: CycleResult::Failed {
                    reason: truncate_chars(&error.cause_message, 300),
                },
                tokens: tokens.max(error.usage.total_tokens),
            },
            Ended::Finished(Ok(InvestigationRunOutcome::Cancelled)) => {
                InvestigationResult::Interrupted { tokens }
            }
            Ended::Finished(Ok(InvestigationRunOutcome::Completed(run))) => {
                let tokens = tokens.max(run.usage.total_tokens);
                match run.report {
                    Some(report) => InvestigationResult::Report {
                        report: Box::new(report),
                        tokens,
                    },
                    None if budget.tokens_exhausted()
                        || budget.tool_calls_used()
                            >= objective.budget.max_tool_calls_per_cycle =>
                    {
                        InvestigationResult::Ended {
                            result: CycleResult::BudgetExhausted {
                                reason: "budget exhausted before a report was submitted"
                                    .to_string(),
                            },
                            tokens,
                        }
                    }
                    None => InvestigationResult::Ended {
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

    async fn renew_lease(&self, ledger: &mut StoredLedger) {
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

    async fn decide(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
        observed: &ObserveOutput,
        report: &InvestigationReport,
        tokens: u64,
    ) -> Result<StoredLedger, ObjectiveCycleError> {
        let now = self.deps.clock.now();
        let policy = apply_report_policy(report);
        let changes = plan_changes(&PlanChangesInput {
            objective,
            existing: &observed.items,
            proposals: &policy.proposals,
        });
        let context = ApplyContext {
            run_id: &active.run_id,
            now,
            report,
        };
        let mut touched: Vec<String> = Vec::new();
        let mut skipped = Vec::new();
        for change in &changes {
            match change {
                PlannedChange::Update {
                    record_key,
                    proposal,
                } => {
                    let Some(item) = observed
                        .items
                        .iter()
                        .find(|item| item.record.key == *record_key)
                    else {
                        continue;
                    };
                    let saved = self
                        .deps
                        .store
                        .mutate_work_item(item, |state| apply_proposal(state, proposal, &context))
                        .await?;
                    self.append_cycle_log(&saved, active, proposal_log(proposal))
                        .await?;
                    touched.push(saved.record.key.clone());
                }
                PlannedChange::Create { proposal } => {
                    let Some(state) = new_work_item_state(objective, proposal, &context) else {
                        continue;
                    };
                    let saved = self.deps.store.create_work_item_once(&state).await?;
                    self.append_cycle_log(&saved, active, proposal_log(proposal))
                        .await?;
                    touched.push(saved.record.key.clone());
                }
                PlannedChange::Skipped { key, reason } => {
                    skipped.push(format!("`{key}`: {reason}"));
                }
            }
        }
        for item in observed.items.iter().filter(|item| {
            !item.state.pending_feedback().is_empty() && !touched.contains(&item.record.key)
        }) {
            let run_id = active.run_id.clone();
            let saved = self
                .deps
                .store
                .mutate_work_item(item, |state| {
                    for note in &mut state.feedback {
                        if note.applied_in_run.is_none() {
                            note.applied_in_run = Some(run_id.clone());
                        }
                    }
                })
                .await?;
            self.append_cycle_log(
                &saved,
                active,
                "Human feedback was included in this run; no update to this item was proposed."
                    .to_string(),
            )
            .await?;
        }
        touched.sort_unstable();
        touched.dedup();

        let mut summary = truncate_chars(report.summary.trim(), 1_000);
        if !policy.notes.is_empty() {
            summary.push_str(&format!(
                " [report policy adjusted {} item(s)]",
                policy.notes.len()
            ));
        }
        if !skipped.is_empty() {
            summary.push_str(&format!(" [skipped: {}]", skipped.join("; ")));
        }
        let audit = policy
            .notes
            .iter()
            .chain(policy.rejected_knowledge.iter().map(|note| note as &String))
            .cloned()
            .collect::<Vec<_>>();
        if !audit.is_empty() {
            self.deps
                .store
                .repository()
                .append_comment(
                    &ledger.record,
                    WorkStateComment {
                        kind: WorkStateCommentKind::CycleLog,
                        body: format!(
                            "Run `{}` report policy and knowledge audit:\n{}",
                            active.run_id,
                            audit
                                .iter()
                                .map(|note| format!("- {}", truncate_chars(note, 300)))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ),
                    },
                )
                .await?;
        }

        let knowledge: Vec<KnowledgeDraft> = if objective.permissions.save_knowledge {
            policy.knowledge.clone()
        } else {
            Vec::new()
        };
        let no_new_findings = touched.is_empty();
        let (saved, ()) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                let usage = state.usage_for(now.date_naive());
                usage.llm_tokens += tokens;
                usage.cycles += 1;
                if let Some(cycle) = state.active_cycle.as_mut() {
                    cycle.phase = CyclePhase::Reporting;
                    cycle.pending_notifications.clone_from(&touched);
                    cycle.pending_knowledge.clone_from(&knowledge);
                    cycle.summary.clone_from(&summary);
                    cycle.no_new_findings = no_new_findings;
                    cycle.llm_tokens += tokens;
                }
                state.updated_at = now;
            })
            .await?;
        Ok(saved)
    }

    async fn append_cycle_log(
        &self,
        item: &StoredWorkItem,
        active: &ActiveCycle,
        body: String,
    ) -> Result<(), WorkStateError> {
        self.deps
            .store
            .repository()
            .append_comment(
                &item.record,
                WorkStateComment {
                    kind: WorkStateCommentKind::CycleLog,
                    body: format!("Run `{}`: {body}", active.run_id),
                },
            )
            .await
    }

    async fn report(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
    ) -> Result<StoredLedger, ReportError> {
        let now = self.deps.clock.now();
        let items = self
            .deps
            .store
            .repository()
            .list_work_items(&objective.id)
            .await?;
        let mut candidates = active.pending_notifications.clone();
        candidates.extend(
            items
                .iter()
                .filter(|item| item.state.status.is_open())
                .filter(|item| {
                    item.state.notification.last_digest.as_deref()
                        != Some(notification_digest(item).as_str())
                })
                .map(|item| item.record.key.clone()),
        );
        let decision = compose_report(&ComposeReportInput {
            objective,
            ledger: &ledger.state,
            ledger_url: &ledger.record.url,
            items: &items,
            candidate_keys: &candidates,
            summary: &active.summary,
            run_id: &active.run_id,
            now,
        });

        let mut posted_key = None;
        match decision {
            ReportDecision::Post { text, item_keys } if objective.permissions.post_reports => {
                let key = report_key(objective.id.as_str(), &active.run_id);
                let already = self
                    .deps
                    .report_port
                    .find_posted_report(&objective.report.channel_id, &key)
                    .await
                    .map_err(ReportError::Slack)?;
                if already.is_none() {
                    self.deps
                        .report_port
                        .post_report(ObjectiveReportMessage {
                            channel_id: objective.report.channel_id.clone(),
                            objective_id: objective.id.to_string(),
                            report_key: key.clone(),
                            text,
                        })
                        .await
                        .map_err(ReportError::Slack)?;
                }
                for item in items
                    .iter()
                    .filter(|item| item_keys.contains(&item.record.key))
                {
                    let digest = notification_digest(item);
                    self.deps
                        .store
                        .mutate_work_item(item, |state| {
                            state.notification.last_digest = Some(digest.clone());
                            state.notification.last_notified_at = Some(now);
                        })
                        .await?;
                }
                posted_key = Some(key);
            }
            ReportDecision::Post { .. } | ReportDecision::Nothing => {}
            ReportDecision::Deferred { reason } => {
                self.log(
                    "objective_cycle_report_deferred",
                    objective,
                    active,
                    Some(&reason),
                );
            }
        }

        let (saved, ()) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                if let Some(key) = &posted_key
                    && !state.reports.recent_report_keys.contains(key)
                {
                    state.remember_report_key(key.clone(), now);
                    state.usage_for(now.date_naive()).reports_posted += 1;
                }
                if let Some(cycle) = state.active_cycle.as_mut() {
                    cycle.phase = CyclePhase::Learning;
                }
                state.updated_at = now;
            })
            .await?;
        Ok(saved)
    }

    async fn learn(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
    ) -> Result<StoredLedger, ObjectiveCycleError> {
        let Some(canvas) = self
            .deps
            .task_resources
            .canvas_memory_port
            .as_ref()
            .filter(|_| objective.permissions.save_knowledge)
        else {
            return Ok(ledger.clone());
        };
        let mut learned = Vec::new();
        for draft in active.pending_knowledge.iter().take(MAX_FACTS_PER_CYCLE) {
            let fingerprint = stable_fingerprint(&draft.fact.to_lowercase());
            if ledger.state.has_learned(&fingerprint) {
                continue;
            }
            let result = canvas
                .append_memory(AppendSlackCanvasMemoryInput {
                    visibility: if draft.shared {
                        SlackCanvasMemoryVisibility::Shared
                    } else {
                        SlackCanvasMemoryVisibility::Channel
                    },
                    channel_id: objective.report.channel_id.clone(),
                    channel_name: None,
                    source_message_ts: String::new(),
                    source_url: Some(ledger.record.url.clone()),
                    fact: draft.fact.clone(),
                    evidence: draft.evidence.clone(),
                    scope: draft.scope.clone(),
                })
                .await;
            match result {
                Ok(()) => learned.push(fingerprint),
                Err(error) => self.log(
                    "objective_cycle_learn_failed",
                    objective,
                    active,
                    Some(&error.message),
                ),
            }
        }
        if learned.is_empty() {
            return Ok(ledger.clone());
        }
        let (saved, ()) = self
            .deps
            .store
            .mutate_ledger(ledger, |state| {
                for fingerprint in &learned {
                    if !state.has_learned(fingerprint) {
                        state.remember_learned(fingerprint.clone());
                    }
                }
            })
            .await?;
        Ok(saved)
    }

    async fn finalize(
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
                state.record_cycle(CycleRecord {
                    run_id: active.run_id.clone(),
                    scheduled_for: active.scheduled_for,
                    started_at: active.started_at,
                    finished_at: now,
                    attempts: active.attempt,
                    result: final_result.clone(),
                    llm_tokens: active.llm_tokens + extra_tokens,
                    summary: active.summary.clone(),
                    touched_items: active.pending_notifications.clone(),
                });
                state.active_cycle = None;
                if (!failed || give_up)
                    && let Some(slot) = active.scheduled_for
                {
                    state.last_scheduled_for =
                        Some(state.last_scheduled_for.map_or(slot, |last| last.max(slot)));
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

    /// Posts a one-off notice for cycles that ended without results humans should know about.
    async fn notify_terminal_result(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        result: &CycleResult,
    ) {
        let message = match result {
            CycleResult::Abandoned { reason } => format!(
                "*Objective `{}`*: a cycle was abandoned — {reason}. Reili will try again at the next scheduled time. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            CycleResult::BudgetExhausted { reason } => format!(
                "*Objective `{}`*: a cycle stopped on its budget — {reason}. Partial results were not recorded as findings. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            CycleResult::Skipped { reason } if reason.contains("budget") => format!(
                "*Objective `{}`*: skipped a cycle — {reason}. <{}|objective ledger>",
                objective.id, ledger.record.url
            ),
            _ => return,
        };
        if !objective.permissions.post_reports {
            return;
        }
        let today = self.deps.clock.now().date_naive();
        let key = format!(
            "{}:notice:{}:{}",
            objective.id,
            today,
            stable_fingerprint(&message)
        );
        if ledger.state.reports.recent_report_keys.contains(&key) {
            return;
        }
        let posted = self
            .deps
            .report_port
            .find_posted_report(&objective.report.channel_id, &key)
            .await;
        if !matches!(posted, Ok(None)) {
            return;
        }
        let post = self
            .deps
            .report_port
            .post_report(ObjectiveReportMessage {
                channel_id: objective.report.channel_id.clone(),
                objective_id: objective.id.to_string(),
                report_key: key.clone(),
                text: message,
            })
            .await;
        if post.is_ok() {
            let now = self.deps.clock.now();
            let _ = self
                .deps
                .store
                .mutate_ledger(ledger, |state| state.remember_report_key(key.clone(), now))
                .await;
        }
    }

    async fn record_usage(
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

enum ReportError {
    WorkState(WorkStateError),
    Slack(PortError),
}

impl From<WorkStateError> for ReportError {
    fn from(error: WorkStateError) -> Self {
        Self::WorkState(error)
    }
}

fn missing_cycle(objective: &ObjectiveDefinition) -> ObjectiveCycleError {
    ObjectiveCycleError::WorkState(WorkStateError::Other(PortError::new(format!(
        "active cycle for `{}` disappeared from the ledger",
        objective.id
    ))))
}

fn acquire(
    state: &mut ObjectiveLedgerState,
    objective: &ObjectiveDefinition,
    start: &CycleStart,
    instance_id: &str,
    run_id: &str,
    now: DateTime<Utc>,
    lease: Duration,
) -> Acquisition {
    if let Some(active) = state.active_cycle.as_mut() {
        if active.lease_is_live(now) {
            return Acquisition::Busy(format!("cycle {} is already running", active.run_id));
        }
        if active.attempt >= objective.budget.max_cycle_attempts {
            let abandoned = active.clone();
            state.record_cycle(CycleRecord {
                run_id: abandoned.run_id.clone(),
                scheduled_for: abandoned.scheduled_for,
                started_at: abandoned.started_at,
                finished_at: now,
                attempts: abandoned.attempt,
                result: CycleResult::Abandoned {
                    reason: format!(
                        "interrupted {} time(s) in phase {:?}",
                        abandoned.attempt, abandoned.phase
                    ),
                },
                llm_tokens: abandoned.llm_tokens,
                summary: abandoned.summary.clone(),
                touched_items: abandoned.pending_notifications.clone(),
            });
            if let Some(slot) = abandoned.scheduled_for {
                state.last_scheduled_for =
                    Some(state.last_scheduled_for.map_or(slot, |last| last.max(slot)));
            }
            state.active_cycle = None;
            state.updated_at = now;
            return Acquisition::Finished(CycleResult::Abandoned {
                reason: format!("interrupted {} time(s)", abandoned.attempt),
            });
        }
        active.attempt += 1;
        active.lease_holder = instance_id.to_string();
        active.lease_expires_at = now + lease;
        state.updated_at = now;
        return Acquisition::Run(active.clone());
    }

    let CycleStart::New { scheduled_for } = start else {
        return Acquisition::Busy("no interrupted cycle to resume".to_string());
    };
    let today = now.date_naive();
    if state.tokens_used_on(today) >= objective.budget.max_llm_tokens_per_day {
        let reason = format!(
            "daily LLM token budget ({}) exhausted",
            objective.budget.max_llm_tokens_per_day
        );
        state.record_cycle(CycleRecord {
            run_id: run_id.to_string(),
            scheduled_for: *scheduled_for,
            started_at: now,
            finished_at: now,
            attempts: 0,
            result: CycleResult::Skipped {
                reason: reason.clone(),
            },
            llm_tokens: 0,
            summary: String::new(),
            touched_items: Vec::new(),
        });
        if let Some(slot) = scheduled_for {
            state.last_scheduled_for = Some(
                state
                    .last_scheduled_for
                    .map_or(*slot, |last| last.max(*slot)),
            );
        }
        state.updated_at = now;
        return Acquisition::Finished(CycleResult::Skipped { reason });
    }
    let active = ActiveCycle {
        run_id: run_id.to_string(),
        scheduled_for: *scheduled_for,
        started_at: now,
        attempt: 1,
        phase: CyclePhase::Observing,
        lease_holder: instance_id.to_string(),
        lease_expires_at: now + lease,
        pending_notifications: Vec::new(),
        pending_knowledge: Vec::new(),
        summary: String::new(),
        no_new_findings: false,
        llm_tokens: 0,
    };
    state.active_cycle = Some(active.clone());
    state.updated_at = now;
    Acquisition::Run(active)
}

fn proposal_log(proposal: &reili_core::work::WorkItemProposal) -> String {
    let hypotheses = proposal
        .hypotheses
        .iter()
        .map(|hypothesis| format!("{}={}", hypothesis.id, hypothesis.status.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "status proposal `{}`; hypotheses [{}]; {} finding(s).",
        proposal.proposed_status.as_str(),
        hypotheses,
        proposal.findings.len()
    )
}

struct DiscardProgressEvents;

#[async_trait]
impl TaskProgressEventPort for DiscardProgressEvents {
    async fn publish(&self, _input: TaskProgressEventInput) -> Result<(), PortError> {
        Ok(())
    }
}
