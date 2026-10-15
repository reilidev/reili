//! Decide: map gated proposals onto existing work items (stable keys, then fuzzy matching),
//! enforce per-goal limits, and merge investigation results into durable state while respecting
//! human decisions.

use chrono::{DateTime, Utc};
use reili_core::goal::GoalDefinition;
use reili_core::work::{
    EvidenceRef, Finding, HumanDecisionRequest, Hypothesis, InvestigationReport, NotificationState,
    Recommendation, StatusActor, StoredWorkItem, VerificationRecord, WORK_ITEM_SCHEMA_VERSION,
    WorkItemKey, WorkItemProposal, WorkItemState, WorkItemStatus, sanitize::sanitize_for_storage,
    token_jaccard,
};

const KEY_SIMILARITY_THRESHOLD: f64 = 0.75;
const TITLE_SIMILARITY_THRESHOLD: f64 = 0.8;
const STATEMENT_SIMILARITY_THRESHOLD: f64 = 0.8;
const MAX_TITLE_CHARS: usize = 200;
const MAX_TEXT_CHARS: usize = 2_000;
const MAX_SHORT_TEXT_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub enum PlannedChange {
    Update {
        issue_number: u64,
        proposal: WorkItemProposal,
    },
    Create {
        proposal: WorkItemProposal,
    },
    Skipped {
        key: String,
        reason: String,
    },
}

pub struct PlanChangesInput<'a> {
    pub goal: &'a GoalDefinition,
    pub existing: &'a [StoredWorkItem],
    pub proposals: &'a [WorkItemProposal],
}

#[must_use]
pub fn plan_changes(input: &PlanChangesInput<'_>) -> Vec<PlannedChange> {
    let mut changes = Vec::new();
    let mut creates = Vec::new();
    let mut claimed_issues = std::collections::BTreeSet::new();

    for proposal in input.proposals {
        let Some(key) = WorkItemKey::normalize(&proposal.key) else {
            changes.push(PlannedChange::Skipped {
                key: proposal.key.clone(),
                reason: "work item key is empty after normalization".to_string(),
            });
            continue;
        };
        match find_match(input.existing, proposal, &key) {
            Some(existing) if claimed_issues.contains(&existing.issue.number) => {
                changes.push(PlannedChange::Skipped {
                    key: key.to_string(),
                    reason: format!(
                        "duplicates another proposal for #{} in the same cycle",
                        existing.issue.number
                    ),
                });
            }
            Some(existing) if existing.state.status == WorkItemStatus::Dismissed => {
                changes.push(PlannedChange::Skipped {
                    key: key.to_string(),
                    reason: format!(
                        "matches #{} which a human dismissed; not reopening without a human request",
                        existing.issue.number
                    ),
                });
            }
            Some(existing) => {
                claimed_issues.insert(existing.issue.number);
                changes.push(PlannedChange::Update {
                    issue_number: existing.issue.number,
                    proposal: proposal.clone(),
                });
            }
            None => creates.push(proposal.clone()),
        }
    }

    creates.sort_by_key(|proposal| proposal.severity);
    let mut open_count = input
        .existing
        .iter()
        .filter(|item| item.state.status.is_open())
        .count();
    let mut created = 0_u32;
    let mut created_keys = std::collections::BTreeSet::new();
    for proposal in creates {
        let key = WorkItemKey::normalize(&proposal.key)
            .map_or_else(|| proposal.key.clone(), |key| key.to_string());
        if !created_keys.insert(key.clone()) {
            changes.push(PlannedChange::Skipped {
                key,
                reason: "duplicate key within the same report".to_string(),
            });
            continue;
        }
        let reason = if created >= input.goal.budget.max_new_work_items_per_cycle {
            Some(format!(
                "max_new_work_items_per_cycle ({}) reached",
                input.goal.budget.max_new_work_items_per_cycle
            ))
        } else if open_count >= input.goal.budget.max_open_work_items as usize {
            Some(format!(
                "max_open_work_items ({}) reached",
                input.goal.budget.max_open_work_items
            ))
        } else {
            None
        };
        match reason {
            Some(reason) => changes.push(PlannedChange::Skipped { key, reason }),
            None => {
                created += 1;
                open_count += 1;
                changes.push(PlannedChange::Create { proposal });
            }
        }
    }
    changes
}

fn find_match<'a>(
    existing: &'a [StoredWorkItem],
    proposal: &WorkItemProposal,
    key: &WorkItemKey,
) -> Option<&'a StoredWorkItem> {
    if let Some(number) = proposal.existing_issue_number
        && let Some(found) = existing.iter().find(|item| item.issue.number == number)
    {
        return Some(found);
    }
    if let Some(found) = existing.iter().find(|item| &item.state.key == key) {
        return Some(found);
    }
    existing
        .iter()
        .map(|item| {
            let score = item.state.key.similarity(key).max(
                if token_jaccard(&item.state.title, &proposal.title) >= TITLE_SIMILARITY_THRESHOLD {
                    1.0
                } else {
                    0.0
                },
            );
            (item, score)
        })
        .filter(|(_, score)| *score >= KEY_SIMILARITY_THRESHOLD)
        .max_by(|left, right| {
            left.1.total_cmp(&right.1).then(
                left.0
                    .state
                    .status
                    .is_open()
                    .cmp(&right.0.state.status.is_open()),
            )
        })
        .map(|(item, _)| item)
}

/// Context for merging one cycle's results into a work item.
pub struct ApplyContext<'a> {
    pub run_id: &'a str,
    pub now: DateTime<Utc>,
    pub report: &'a InvestigationReport,
    pub verifications: &'a [VerificationRecord],
}

impl ApplyContext<'_> {
    fn scoped_id(&self, id: &str) -> String {
        format!("{}-{}", short_run_id(self.run_id), id)
    }
}

#[must_use]
pub fn short_run_id(run_id: &str) -> String {
    run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(6)
        .collect()
}

#[must_use]
pub fn new_work_item_state(
    goal: &GoalDefinition,
    proposal: &WorkItemProposal,
    context: &ApplyContext<'_>,
) -> Option<WorkItemState> {
    let key = WorkItemKey::normalize(&proposal.key)?;
    let mut state = WorkItemState {
        schema_version: WORK_ITEM_SCHEMA_VERSION,
        key,
        goal_id: goal.id.clone(),
        title: String::new(),
        kind: proposal.kind,
        severity: proposal.severity,
        status: WorkItemStatus::Investigating,
        problem_statement: String::new(),
        plan: Vec::new(),
        hypotheses: Vec::new(),
        findings: Vec::new(),
        recommendation: None,
        evidence: Vec::new(),
        verifications: Vec::new(),
        open_questions: Vec::new(),
        human_decision: None,
        next_actions: Vec::new(),
        outcome: None,
        feedback: Vec::new(),
        history: vec![reili_core::work::StatusChange {
            at: context.now,
            from: None,
            to: WorkItemStatus::Investigating,
            actor: StatusActor::Reili,
            reason: format!("discovered in cycle {}", context.run_id),
        }],
        last_run_id: None,
        created_at: context.now,
        updated_at: context.now,
        last_processed_comment_id: None,
        notification: NotificationState::default(),
        rerun_requested: false,
        revision: 0,
    };
    apply_proposal(&mut state, proposal, context);
    Some(state)
}

/// Merges a gated proposal into `state`. Human-owned states win: `monitoring` stays until outcome
/// evaluation decides, `needs-human` waits for a human answer, and `dismissed` is never reopened.
pub fn apply_proposal(
    state: &mut WorkItemState,
    proposal: &WorkItemProposal,
    context: &ApplyContext<'_>,
) {
    let text = |value: &str, max: usize| sanitize_for_storage(value, max);
    if !proposal.title.trim().is_empty() {
        state.title = text(&proposal.title, MAX_TITLE_CHARS);
    }
    state.kind = proposal.kind;
    state.severity = proposal.severity;
    if !proposal.problem_statement.trim().is_empty() {
        state.problem_statement = text(&proposal.problem_statement, MAX_TEXT_CHARS);
    }
    if !proposal.plan.is_empty() {
        state.plan = sanitize_list(&proposal.plan);
    }
    state.open_questions = sanitize_list(&proposal.open_questions);
    state.next_actions = sanitize_list(&proposal.next_actions);

    merge_evidence(state, proposal, context);
    merge_verifications(state, proposal, context);
    merge_hypotheses(state, &proposal.hypotheses, context);
    merge_findings(state, &proposal.findings, context);
    if let Some(recommendation) = &proposal.recommendation {
        state.recommendation = Some(sanitize_recommendation(recommendation));
    }

    let target = proposal.proposed_status;
    let reason = format!("cycle {}", context.run_id);
    match state.status {
        WorkItemStatus::Dismissed => {}
        WorkItemStatus::Monitoring => {}
        WorkItemStatus::NeedsHuman if target != WorkItemStatus::Resolved => {}
        WorkItemStatus::Resolved => {
            if target.is_open() {
                state.transition(
                    target,
                    StatusActor::Reili,
                    format!("recurred; {reason}"),
                    context.now,
                );
            }
        }
        _ => state.transition(target, StatusActor::Reili, reason, context.now),
    }
    state.human_decision = match (&state.status, &proposal.human_decision_needed) {
        (WorkItemStatus::NeedsHuman, Some(question)) if !question.trim().is_empty() => {
            Some(HumanDecisionRequest {
                question: text(question, MAX_SHORT_TEXT_CHARS),
                requested_at: state
                    .human_decision
                    .as_ref()
                    .map_or(context.now, |existing| existing.requested_at),
            })
        }
        (WorkItemStatus::NeedsHuman, _) => state.human_decision.clone(),
        _ => None,
    };

    for note in &mut state.feedback {
        if note.applied_in_run.is_none() {
            note.applied_in_run = Some(context.run_id.to_string());
        }
    }
    state.rerun_requested = false;
    state.last_run_id = Some(context.run_id.to_string());
    state.updated_at = context.now;
}

fn sanitize_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| sanitize_for_storage(value, MAX_SHORT_TEXT_CHARS))
        .filter(|value| !value.is_empty())
        .collect()
}

fn sanitize_recommendation(recommendation: &Recommendation) -> Recommendation {
    Recommendation {
        action: sanitize_for_storage(&recommendation.action, MAX_TEXT_CHARS),
        rationale: sanitize_for_storage(&recommendation.rationale, MAX_TEXT_CHARS),
        expected_effect: sanitize_for_storage(
            &recommendation.expected_effect,
            MAX_SHORT_TEXT_CHARS,
        ),
        risk: sanitize_for_storage(&recommendation.risk, MAX_SHORT_TEXT_CHARS),
        outcome_metric: recommendation.outcome_metric.clone(),
    }
}

fn referenced_evidence_ids(proposal: &WorkItemProposal) -> Vec<String> {
    let mut ids: Vec<String> = proposal.evidence_ids.clone();
    for hypothesis in &proposal.hypotheses {
        ids.extend(hypothesis.evidence_ids.iter().cloned());
    }
    for finding in &proposal.findings {
        ids.extend(finding.evidence_ids.iter().cloned());
    }
    ids.sort();
    ids.dedup();
    ids
}

fn merge_evidence(
    state: &mut WorkItemState,
    proposal: &WorkItemProposal,
    context: &ApplyContext<'_>,
) {
    for id in referenced_evidence_ids(proposal) {
        let Some(evidence) = context.report.evidence_by_id(&id) else {
            continue;
        };
        let duplicate = state.evidence.iter().any(|existing| {
            existing.url.is_some()
                && existing.url == evidence.url
                && existing.description == evidence.description
        });
        if duplicate {
            continue;
        }
        state.evidence.push(EvidenceRef {
            id: context.scoped_id(&evidence.id),
            source: evidence.source,
            url: evidence.url.clone(),
            description: sanitize_for_storage(&evidence.description, MAX_SHORT_TEXT_CHARS),
            observed_at: evidence.observed_at,
            time_range: evidence.time_range.clone(),
            query: evidence
                .query
                .as_deref()
                .map(|query| sanitize_for_storage(query, MAX_SHORT_TEXT_CHARS)),
        });
    }
}

fn merge_verifications(
    state: &mut WorkItemState,
    proposal: &WorkItemProposal,
    context: &ApplyContext<'_>,
) {
    let mut ids: Vec<&String> = proposal
        .hypotheses
        .iter()
        .flat_map(|hypothesis| hypothesis.verification_ids.iter())
        .chain(
            proposal
                .findings
                .iter()
                .flat_map(|finding| finding.verification_ids.iter()),
        )
        .collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let Some(record) = context.verifications.iter().find(|record| &record.id == id) else {
            continue;
        };
        let scoped = context.scoped_id(&record.id);
        if state
            .verifications
            .iter()
            .any(|existing| existing.id == scoped)
        {
            continue;
        }
        let mut record = record.clone();
        record.id = scoped;
        record.claim = sanitize_for_storage(&record.claim, MAX_SHORT_TEXT_CHARS);
        record.detail = sanitize_for_storage(&record.detail, MAX_SHORT_TEXT_CHARS);
        state.verifications.push(record);
    }
}

fn scope_ids(ids: &[String], context: &ApplyContext<'_>) -> Vec<String> {
    ids.iter().map(|id| context.scoped_id(id)).collect()
}

fn merge_hypotheses(
    state: &mut WorkItemState,
    hypotheses: &[Hypothesis],
    context: &ApplyContext<'_>,
) {
    for hypothesis in hypotheses {
        let statement = sanitize_for_storage(&hypothesis.statement, MAX_SHORT_TEXT_CHARS);
        let verification_ids = scope_ids(&hypothesis.verification_ids, context);
        let evidence_ids = scope_ids(&hypothesis.evidence_ids, context);
        let rationale = sanitize_for_storage(&hypothesis.rationale, MAX_SHORT_TEXT_CHARS);
        match state.hypotheses.iter_mut().find(|existing| {
            token_jaccard(&existing.statement, &statement) >= STATEMENT_SIMILARITY_THRESHOLD
        }) {
            Some(existing) => {
                // A previously decided hypothesis keeps its status unless new verifications
                // decided it again.
                if !verification_ids.is_empty()
                    || !matches!(
                        existing.status,
                        reili_core::work::HypothesisStatus::Supported
                            | reili_core::work::HypothesisStatus::Refuted
                    )
                {
                    existing.status = hypothesis.status;
                }
                existing.verification_ids.extend(verification_ids);
                existing.verification_ids.dedup();
                existing.evidence_ids.extend(evidence_ids);
                existing.evidence_ids.dedup();
                if !rationale.is_empty() {
                    existing.rationale = rationale;
                }
            }
            None => state.hypotheses.push(Hypothesis {
                id: context.scoped_id(&hypothesis.id),
                statement,
                status: hypothesis.status,
                verification_ids,
                evidence_ids,
                rationale,
            }),
        }
    }
}

fn merge_findings(state: &mut WorkItemState, findings: &[Finding], context: &ApplyContext<'_>) {
    for finding in findings {
        let statement = sanitize_for_storage(&finding.statement, MAX_SHORT_TEXT_CHARS);
        let verification_ids = scope_ids(&finding.verification_ids, context);
        let evidence_ids = scope_ids(&finding.evidence_ids, context);
        match state.findings.iter_mut().find(|existing| {
            token_jaccard(&existing.statement, &statement) >= STATEMENT_SIMILARITY_THRESHOLD
        }) {
            Some(existing) => {
                existing.verified = finding.verified;
                existing.verification_ids = verification_ids;
                existing.evidence_ids = evidence_ids;
            }
            None => state.findings.push(Finding {
                statement,
                verified: finding.verified,
                verification_ids,
                evidence_ids,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use reili_core::goal::GoalDefinition;
    use reili_core::work::{
        HypothesisStatus, IssueRef, Severity, StoredWorkItem, VerificationStatus, WorkItemKey,
        WorkItemStatus, test_support::sample_work_item,
    };

    use super::{
        ApplyContext, PlanChangesInput, PlannedChange, apply_proposal, new_work_item_state,
        plan_changes,
    };
    use crate::goal::gate::tests::{proposal, record, report};
    use crate::goal::tests_support::sample_goal;

    fn stored(number: u64, key: &str, status: WorkItemStatus) -> StoredWorkItem {
        let mut state = sample_work_item(key);
        state.status = status;
        StoredWorkItem {
            issue: IssueRef {
                number,
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
            state,
            human_notes: String::new(),
            observed_status: status,
            remote_updated_at: Utc.timestamp_opt(0, 0).single().expect("time"),
        }
    }

    fn goal() -> GoalDefinition {
        sample_goal("checkout")
    }

    #[test]
    fn matches_existing_items_by_key_and_similarity() {
        let existing = vec![stored(
            7,
            "checkout:after-deploy-5xx",
            WorkItemStatus::Investigating,
        )];
        let goal = goal();
        let proposals = vec![proposal(HypothesisStatus::Proposed, &[])];

        let changes = plan_changes(&PlanChangesInput {
            goal: &goal,
            existing: &existing,
            proposals: &proposals,
        });

        assert!(matches!(
            changes.as_slice(),
            [PlannedChange::Update {
                issue_number: 7,
                ..
            }]
        ));
    }

    #[test]
    fn respects_human_dismissal() {
        let existing = vec![stored(
            7,
            "checkout:5xx-after-deploy",
            WorkItemStatus::Dismissed,
        )];
        let goal = goal();
        let proposals = vec![proposal(HypothesisStatus::Proposed, &[])];

        let changes = plan_changes(&PlanChangesInput {
            goal: &goal,
            existing: &existing,
            proposals: &proposals,
        });

        assert!(matches!(
            changes.as_slice(),
            [PlannedChange::Skipped { .. }]
        ));
    }

    #[test]
    fn enforces_new_item_limits_by_severity() {
        let mut goal = goal();
        goal.budget.max_new_work_items_per_cycle = 1;
        let mut low = proposal(HypothesisStatus::Proposed, &[]);
        low.key = "a:low".to_string();
        low.title = "low thing".to_string();
        low.severity = Severity::Low;
        let mut critical = proposal(HypothesisStatus::Proposed, &[]);
        critical.key = "b:critical".to_string();
        critical.title = "critical thing".to_string();
        critical.severity = Severity::Critical;

        let changes = plan_changes(&PlanChangesInput {
            goal: &goal,
            existing: &[],
            proposals: &[low, critical],
        });

        assert!(
            matches!(&changes[0], PlannedChange::Create { proposal } if proposal.key == "b:critical")
        );
        assert!(
            matches!(&changes[1], PlannedChange::Skipped { reason, .. } if reason.contains("max_new_work_items_per_cycle"))
        );
    }

    #[test]
    fn new_state_scopes_ids_and_keeps_only_cited_verifications() {
        let goal = goal();
        let gated = proposal(HypothesisStatus::Supported, &["V1"]);
        let report = report(vec![gated.clone()]);
        let verifications = vec![
            record("V1", VerificationStatus::Passed, true),
            record("V2", VerificationStatus::Failed, true),
        ];
        let context = ApplyContext {
            run_id: "run-abc123",
            now: Utc.timestamp_opt(1_760_000_000, 0).single().expect("time"),
            report: &report,
            verifications: &verifications,
        };

        let state = new_work_item_state(&goal, &gated, &context).expect("state");

        assert_eq!(state.status, WorkItemStatus::NeedsHuman);
        assert_eq!(state.verifications.len(), 1);
        assert_eq!(state.verifications[0].id, "runabc-V1");
        assert_eq!(
            state.hypotheses[0].verification_ids,
            vec!["runabc-V1".to_string()]
        );
        assert_eq!(state.evidence.len(), 1);
        assert_eq!(
            state
                .human_decision
                .as_ref()
                .map(|decision| decision.question.as_str()),
            Some("Roll back deploy 42?")
        );
    }

    #[test]
    fn monitoring_and_needs_human_states_are_preserved() {
        let report = report(Vec::new());
        let context = ApplyContext {
            run_id: "run-2",
            now: Utc.timestamp_opt(1_760_000_000, 0).single().expect("time"),
            report: &report,
            verifications: &[],
        };
        let mut gated = proposal(HypothesisStatus::Proposed, &[]);
        gated.proposed_status = WorkItemStatus::Investigating;
        let mut monitoring = sample_work_item("checkout:5xx-after-deploy");
        monitoring.status = WorkItemStatus::Monitoring;

        apply_proposal(&mut monitoring, &gated, &context);

        assert_eq!(monitoring.status, WorkItemStatus::Monitoring);
        assert_eq!(
            monitoring.key,
            WorkItemKey::normalize("checkout:5xx-after-deploy").expect("key")
        );
    }

    #[test]
    fn merges_repeated_hypotheses_instead_of_duplicating() {
        let report = report(Vec::new());
        let context = ApplyContext {
            run_id: "run-3",
            now: Utc.timestamp_opt(1_760_000_000, 0).single().expect("time"),
            report: &report,
            verifications: &[],
        };
        let gated = proposal(HypothesisStatus::Proposed, &[]);
        let mut state = sample_work_item("checkout:5xx-after-deploy");

        apply_proposal(&mut state, &gated, &context);
        apply_proposal(&mut state, &gated, &context);

        assert_eq!(state.hypotheses.len(), 1);
        assert_eq!(state.findings.len(), 1);
    }
}
