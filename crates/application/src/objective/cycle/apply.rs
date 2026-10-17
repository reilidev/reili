//! Decide: apply the report (after the report policy) to Work State — update or create work
//! items, record feedback as used, audit policy changes, and checkpoint the ledger for Report.

use reili_core::objective::ObjectiveDefinition;
use reili_core::work::{
    ActiveCycle, CyclePhase, CycleReport, KnowledgeDraft, RecordKey, StoredLedger, StoredWorkItem,
    WorkItemProposal, WorkStateComment, WorkStateCommentKind, WorkStateError,
    sanitize::truncate_chars,
};

use super::super::decide::{
    ApplyContext, PlanChangesInput, PlannedChange, apply_proposal, new_work_item_state,
    plan_changes,
};
use super::super::policy::apply_report_policy;
use super::observe::ObserveOutput;
use super::{ObjectiveCycleError, ObjectiveCycleRunner};

impl ObjectiveCycleRunner {
    pub(super) async fn decide(
        &self,
        objective: &ObjectiveDefinition,
        ledger: &StoredLedger,
        active: &ActiveCycle,
        observed: &ObserveOutput,
        report: &CycleReport,
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
        let mut touched: Vec<RecordKey> = Vec::new();
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
}

fn proposal_log(proposal: &WorkItemProposal) -> String {
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
