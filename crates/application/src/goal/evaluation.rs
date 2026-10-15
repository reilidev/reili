//! Evaluation framework: replayable incident fixtures, deterministic scoring against ground truth,
//! and baseline comparison for prompt/model changes.
//!
//! Two modes share the same fixtures and scorer:
//! - *Recorded replay* (CI, deterministic): a recorded agent report is replayed through the real
//!   report policy and scored.
//! - *Live* (`reili eval`): the configured agent investigates the fixture with its metric data
//!   and incident context in the brief and no live connectors; its report is scored the same way.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reili_core::analysis::SeriesPoint;
use reili_core::error::PortError;
use reili_core::investigation::{
    BriefSection, BriefTrust, InvestigationBrief, InvestigationRunOutcome, InvestigationRunnerPort,
    RunBudget, RunInvestigationInput, RunLimits,
};
use reili_core::task::{
    TaskCancellation, TaskProgressEventInput, TaskProgressEventPort, TaskResources,
};
use reili_core::work::{HypothesisStatus, InvestigationReport};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::policy::{PolicyOutcome, apply_report_policy};
use crate::TaskLogger;

#[derive(Debug, Error)]
pub enum EvaluationError {
    #[error("invalid evaluation case: {0}")]
    InvalidCase(String),
    #[error("case `{case}` has no recorded run to replay")]
    NothingToReplay { case: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSegment {
    /// Segment applies to points strictly before this time; the last segment has none.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    pub value: f64,
    #[serde(default)]
    pub slope_per_hour: f64,
}

/// Compact, deterministic metric generator: piecewise segments plus a fixed noise pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricFixture {
    pub query: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub step_minutes: u32,
    pub segments: Vec<MetricSegment>,
    /// Relative noise amplitude, e.g. 0.02 for ±2%.
    #[serde(default)]
    pub noise: f64,
}

impl MetricFixture {
    #[must_use]
    pub fn points(&self) -> Vec<SeriesPoint> {
        const PATTERN: [f64; 5] = [0.0, 1.0, -1.0, 0.5, -0.5];
        let mut points = Vec::new();
        let mut cursor = self.from;
        let mut index = 0_usize;
        let step = Duration::minutes(i64::from(self.step_minutes.max(1)));
        while cursor <= self.to {
            let mut segment_start = self.from;
            let mut segment = None;
            for candidate in &self.segments {
                if candidate.until.is_none_or(|until| cursor < until) {
                    segment = Some(candidate);
                    break;
                }
                if let Some(until) = candidate.until {
                    segment_start = until;
                }
            }
            if let Some(segment) = segment {
                #[allow(clippy::cast_precision_loss)]
                let hours = (cursor - segment_start).num_minutes() as f64 / 60.0;
                let base = segment.value + segment.slope_per_hour * hours;
                points.push(SeriesPoint {
                    timestamp_ms: cursor.timestamp_millis(),
                    value: base * (1.0 + self.noise * PATTERN[index % PATTERN.len()]),
                });
            }
            cursor += step;
            index += 1;
        }
        points
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDocument {
    pub title: String,
    pub source: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroundTruth {
    /// A hypothesis or finding mentioning any of these identifies the cause.
    pub root_cause_keywords: Vec<String>,
    /// Plausible but wrong explanations; asserting one as a supported hypothesis or a finding is a
    /// false positive.
    #[serde(default)]
    pub decoy_keywords: Vec<String>,
    pub expect_findings: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedRun {
    pub report: InvestigationReport,
    #[serde(default)]
    pub llm_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationCase {
    pub id: String,
    pub description: String,
    pub objective: String,
    #[serde(default)]
    pub now: Option<DateTime<Utc>>,
    #[serde(default)]
    pub context_documents: Vec<ContextDocument>,
    #[serde(default)]
    pub metrics: Vec<MetricFixture>,
    pub ground_truth: GroundTruth,
    #[serde(default)]
    pub recorded: Option<RecordedRun>,
}

pub fn parse_case(json: &str) -> Result<EvaluationCase, EvaluationError> {
    serde_json::from_str(json).map_err(|error| EvaluationError::InvalidCase(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scorecard {
    pub case_id: String,
    pub root_cause_identified: bool,
    /// Decoy explanations recorded as supported hypotheses or findings (false positives).
    pub decoys_confirmed: u32,
    /// Report items the report policy had to adjust.
    pub policy_adjustments: u32,
    pub findings_total: u32,
    pub findings_with_evidence: u32,
    pub false_alarm: bool,
    pub missed: bool,
    pub llm_tokens: u64,
    pub duration_ms: u64,
}

impl Scorecard {
    #[must_use]
    pub fn evidence_coverage(&self) -> f64 {
        if self.findings_total == 0 {
            1.0
        } else {
            f64::from(self.findings_with_evidence) / f64::from(self.findings_total)
        }
    }
}

#[must_use]
pub fn score_case(
    case: &EvaluationCase,
    report: &InvestigationReport,
    policy: &PolicyOutcome,
    llm_tokens: u64,
    duration_ms: u64,
) -> Scorecard {
    let mentions = |text: &str, keywords: &[String]| {
        let lower = text.to_lowercase();
        keywords
            .iter()
            .any(|keyword| lower.contains(&keyword.to_lowercase()))
    };
    let mut identified = false;
    let mut decoys = 0_u32;
    let mut findings_total = 0_u32;
    let mut findings_with_evidence = 0_u32;
    for proposal in &policy.proposals {
        for hypothesis in &proposal.hypotheses {
            let supported = hypothesis.status == HypothesisStatus::Supported;
            if supported
                && mentions(
                    &hypothesis.statement,
                    &case.ground_truth.root_cause_keywords,
                )
            {
                identified = true;
            }
            if supported && mentions(&hypothesis.statement, &case.ground_truth.decoy_keywords) {
                decoys += 1;
            }
        }
        for finding in &proposal.findings {
            findings_total += 1;
            let has_evidence = finding.evidence_ids.iter().any(|id| {
                report
                    .evidence_by_id(id)
                    .is_some_and(|evidence| evidence.url.is_some())
            });
            if has_evidence {
                findings_with_evidence += 1;
            }
            if mentions(&finding.statement, &case.ground_truth.root_cause_keywords) {
                identified = true;
            }
            if mentions(&finding.statement, &case.ground_truth.decoy_keywords) {
                decoys += 1;
            }
        }
    }
    let proposed = !policy.proposals.is_empty();
    Scorecard {
        case_id: case.id.clone(),
        root_cause_identified: identified,
        decoys_confirmed: decoys,
        policy_adjustments: u32::try_from(policy.notes.len()).unwrap_or(u32::MAX),
        findings_total,
        findings_with_evidence,
        false_alarm: !case.ground_truth.expect_findings && proposed,
        missed: case.ground_truth.expect_findings && !proposed,
        llm_tokens,
        duration_ms,
    }
}

/// Deterministically replays a recorded run through the production report policy and scores it.
pub fn replay_recorded_case(case: &EvaluationCase) -> Result<Scorecard, EvaluationError> {
    let recorded = case
        .recorded
        .as_ref()
        .ok_or_else(|| EvaluationError::NothingToReplay {
            case: case.id.clone(),
        })?;
    let started = Instant::now();
    let policy = apply_report_policy(&recorded.report);
    Ok(score_case(
        case,
        &recorded.report,
        &policy,
        recorded.llm_tokens,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    ))
}

/// Renders a metric fixture as a compact table for the brief (offline evaluation has no metric
/// tools; the data is presented to the agent directly).
fn render_metric_fixture(fixture: &MetricFixture) -> String {
    let rows = fixture
        .points()
        .iter()
        .filter_map(|point| {
            DateTime::<Utc>::from_timestamp_millis(point.timestamp_ms)
                .map(|time| format!("{} {:.6}", time.to_rfc3339(), point.value))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("query: `{}`\n{rows}", fixture.query)
}

pub struct LiveEvaluationInput {
    pub case: EvaluationCase,
    pub runner: Arc<dyn InvestigationRunnerPort>,
    pub resources: TaskResources,
    pub limits: RunLimits,
    pub logger: Arc<dyn TaskLogger>,
}

/// Runs the configured agent against a fixture and scores its report.
pub async fn run_live_case(input: LiveEvaluationInput) -> Scorecard {
    let case = input.case;
    let now = case.now.unwrap_or_else(Utc::now);
    let budget = RunBudget::new(input.limits);
    let started = Instant::now();
    let mut sections = vec![BriefSection {
        heading: "Goal".to_string(),
        body: format!(
            "- objective: {}\n- This is an offline evaluation: only the metrics listed below and the incident context are available.",
            case.objective
        ),
        trust: BriefTrust::Trusted,
    }];
    for metric in &case.metrics {
        sections.push(BriefSection {
            heading: "Metric data".to_string(),
            body: render_metric_fixture(metric),
            trust: BriefTrust::Untrusted {
                source: "evaluation fixture".to_string(),
            },
        });
    }
    for document in &case.context_documents {
        sections.push(BriefSection {
            heading: document.title.clone(),
            body: document.body.clone(),
            trust: BriefTrust::Untrusted {
                source: document.source.clone(),
            },
        });
    }
    let result = input
        .runner
        .run(RunInvestigationInput {
            brief: InvestigationBrief {
                run_id: format!("eval-{}", case.id),
                goal_id: "evaluation".to_string(),
                sections,
                now,
                deadline: now + Duration::minutes(30),
            },
            budget: budget.clone(),
            resources: input.resources,
            cancellation: TaskCancellation::new(),
            on_progress_event: Arc::new(IgnoreProgress),
            logger: input.logger,
        })
        .await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (report, tokens) = match result {
        Ok(InvestigationRunOutcome::Completed(run)) => {
            (run.report, budget.tokens_used().max(run.usage.total_tokens))
        }
        Ok(InvestigationRunOutcome::Cancelled) | Err(_) => (None, budget.tokens_used()),
    };
    let report = report.unwrap_or_else(|| InvestigationReport {
        summary: "no report submitted".to_string(),
        observations: Vec::new(),
        evidence: Vec::new(),
        work_items: Vec::new(),
        knowledge_candidates: Vec::new(),
        no_new_findings: true,
    });
    let policy = apply_report_policy(&report);
    score_case(&case, &report, &policy, tokens, duration_ms)
}

struct IgnoreProgress;

#[async_trait]
impl TaskProgressEventPort for IgnoreProgress {
    async fn publish(&self, _input: TaskProgressEventInput) -> Result<(), PortError> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EvaluationThresholds {
    pub min_root_cause_accuracy: f64,
    pub max_false_positive_rate: f64,
    pub max_false_alarm_rate: f64,
    pub min_evidence_coverage: f64,
}

impl Default for EvaluationThresholds {
    fn default() -> Self {
        Self {
            min_root_cause_accuracy: 0.6,
            max_false_positive_rate: 0.0,
            max_false_alarm_rate: 0.0,
            min_evidence_coverage: 0.7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationSummary {
    pub cases: u32,
    /// Share of cases expecting findings whose root cause was identified.
    pub root_cause_accuracy: f64,
    /// Decoy explanations asserted per case.
    pub false_positive_rate: f64,
    pub false_alarm_rate: f64,
    pub miss_rate: f64,
    pub mean_evidence_coverage: f64,
    pub total_policy_adjustments: u32,
    pub total_llm_tokens: u64,
    pub mean_duration_ms: f64,
    pub passed: bool,
    pub failures: Vec<String>,
}

#[must_use]
pub fn summarize(
    scorecards: &[Scorecard],
    expectations: &BTreeMap<String, bool>,
    thresholds: EvaluationThresholds,
) -> EvaluationSummary {
    #[allow(clippy::cast_precision_loss)]
    let ratio = |count: usize, total: usize| {
        if total == 0 {
            0.0
        } else {
            count as f64 / total as f64
        }
    };
    let with_findings: Vec<&Scorecard> = scorecards
        .iter()
        .filter(|card| expectations.get(&card.case_id).copied().unwrap_or(true))
        .collect();
    let without_findings = scorecards.len() - with_findings.len();
    let root_cause_accuracy = ratio(
        with_findings
            .iter()
            .filter(|card| card.root_cause_identified)
            .count(),
        with_findings.len(),
    );
    let false_positive_rate = ratio(
        scorecards
            .iter()
            .map(|card| card.decoys_confirmed as usize)
            .sum(),
        scorecards.len(),
    );
    let false_alarm_rate = ratio(
        scorecards.iter().filter(|card| card.false_alarm).count(),
        without_findings,
    );
    let miss_rate = ratio(
        scorecards.iter().filter(|card| card.missed).count(),
        with_findings.len(),
    );
    #[allow(clippy::cast_precision_loss)]
    let mean_evidence_coverage = if scorecards.is_empty() {
        0.0
    } else {
        scorecards
            .iter()
            .map(Scorecard::evidence_coverage)
            .sum::<f64>()
            / scorecards.len() as f64
    };
    #[allow(clippy::cast_precision_loss)]
    let mean_duration_ms = if scorecards.is_empty() {
        0.0
    } else {
        scorecards
            .iter()
            .map(|card| card.duration_ms as f64)
            .sum::<f64>()
            / scorecards.len() as f64
    };
    let mut failures = Vec::new();
    if root_cause_accuracy < thresholds.min_root_cause_accuracy {
        failures.push(format!(
            "root cause accuracy {root_cause_accuracy:.2} < {:.2}",
            thresholds.min_root_cause_accuracy
        ));
    }
    if false_positive_rate > thresholds.max_false_positive_rate {
        failures.push(format!(
            "false positive rate {false_positive_rate:.2} > {:.2}",
            thresholds.max_false_positive_rate
        ));
    }
    if false_alarm_rate > thresholds.max_false_alarm_rate {
        failures.push(format!(
            "false alarm rate {false_alarm_rate:.2} > {:.2}",
            thresholds.max_false_alarm_rate
        ));
    }
    if mean_evidence_coverage < thresholds.min_evidence_coverage {
        failures.push(format!(
            "evidence coverage {mean_evidence_coverage:.2} < {:.2}",
            thresholds.min_evidence_coverage
        ));
    }
    EvaluationSummary {
        cases: u32::try_from(scorecards.len()).unwrap_or(u32::MAX),
        root_cause_accuracy,
        false_positive_rate,
        false_alarm_rate,
        miss_rate,
        mean_evidence_coverage,
        total_policy_adjustments: scorecards.iter().map(|card| card.policy_adjustments).sum(),
        total_llm_tokens: scorecards.iter().map(|card| card.llm_tokens).sum(),
        mean_duration_ms,
        passed: failures.is_empty(),
        failures,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationComparison {
    pub accuracy_delta: f64,
    pub false_positive_delta: f64,
    pub evidence_coverage_delta: f64,
    pub token_delta: i64,
    pub regressions: Vec<String>,
}

/// Compares a candidate (new prompt/model) against a stored baseline summary.
#[must_use]
pub fn compare_summaries(
    baseline: &EvaluationSummary,
    candidate: &EvaluationSummary,
) -> EvaluationComparison {
    let mut regressions = Vec::new();
    let accuracy_delta = candidate.root_cause_accuracy - baseline.root_cause_accuracy;
    let false_positive_delta = candidate.false_positive_rate - baseline.false_positive_rate;
    let evidence_coverage_delta =
        candidate.mean_evidence_coverage - baseline.mean_evidence_coverage;
    if accuracy_delta < -f64::EPSILON {
        regressions.push(format!(
            "root cause accuracy dropped by {:.2}",
            -accuracy_delta
        ));
    }
    if false_positive_delta > f64::EPSILON {
        regressions.push(format!(
            "false positive rate rose by {false_positive_delta:.2}"
        ));
    }
    if evidence_coverage_delta < -0.05 {
        regressions.push(format!(
            "evidence coverage dropped by {:.2}",
            -evidence_coverage_delta
        ));
    }
    EvaluationComparison {
        accuracy_delta,
        false_positive_delta,
        evidence_coverage_delta,
        token_delta: i64::try_from(candidate.total_llm_tokens).unwrap_or(i64::MAX)
            - i64::try_from(baseline.total_llm_tokens).unwrap_or(i64::MAX),
        regressions,
    }
}

#[must_use]
pub fn render_markdown(
    summary: &EvaluationSummary,
    scorecards: &[Scorecard],
    comparison: Option<&EvaluationComparison>,
) -> String {
    let mut text = format!(
        "# Reili evaluation\n\n**Result:** {}\n\n| metric | value |\n|---|---|\n| cases | {} |\n| root cause accuracy | {:.2} |\n| false positive rate | {:.2} |\n| false alarm rate | {:.2} |\n| miss rate | {:.2} |\n| evidence coverage | {:.2} |\n| policy adjustments | {} |\n| LLM tokens | {} |\n| mean duration (ms) | {:.0} |\n",
        if summary.passed { "PASS" } else { "FAIL" },
        summary.cases,
        summary.root_cause_accuracy,
        summary.false_positive_rate,
        summary.false_alarm_rate,
        summary.miss_rate,
        summary.mean_evidence_coverage,
        summary.total_policy_adjustments,
        summary.total_llm_tokens,
        summary.mean_duration_ms,
    );
    for failure in &summary.failures {
        let _ = writeln!(text, "\n- threshold failed: {failure}");
    }
    text.push_str("\n## Cases\n\n| case | cause found | decoys | adjustments | evidence | false alarm | missed |\n|---|---|---|---|---|---|---|\n");
    for card in scorecards {
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} | {:.2} | {} | {} |",
            card.case_id,
            card.root_cause_identified,
            card.decoys_confirmed,
            card.policy_adjustments,
            card.evidence_coverage(),
            card.false_alarm,
            card.missed
        );
    }
    if let Some(comparison) = comparison {
        let _ = write!(
            text,
            "\n## Against baseline\n\n- accuracy Δ {:+.2}\n- false positive Δ {:+.2}\n- evidence coverage Δ {:+.2}\n- tokens Δ {:+}\n",
            comparison.accuracy_delta,
            comparison.false_positive_delta,
            comparison.evidence_coverage_delta,
            comparison.token_delta
        );
        for regression in &comparison.regressions {
            let _ = writeln!(text, "- regression: {regression}");
        }
    }
    text
}

/// Fixtures shipped with the repository and replayed in CI.
pub const BUNDLED_CASES: &[&str] = &[
    include_str!("../../fixtures/eval/deploy-regression.json"),
    include_str!("../../fixtures/eval/correlation-trap.json"),
    include_str!("../../fixtures/eval/quiet-period.json"),
    include_str!("../../fixtures/eval/gradual-latency.json"),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        BUNDLED_CASES, EvaluationThresholds, compare_summaries, parse_case, render_markdown,
        replay_recorded_case, summarize,
    };

    fn replay_all() -> (Vec<super::Scorecard>, BTreeMap<String, bool>) {
        let mut cards = Vec::new();
        let mut expectations = BTreeMap::new();
        for json in BUNDLED_CASES {
            let case = parse_case(json).expect("parse case");
            expectations.insert(case.id.clone(), case.ground_truth.expect_findings);
            cards.push(replay_recorded_case(&case).expect("replay"));
        }
        (cards, expectations)
    }

    #[test]
    fn bundled_cases_meet_quality_thresholds() {
        let (cards, expectations) = replay_all();

        let summary = summarize(&cards, &expectations, EvaluationThresholds::default());

        assert!(summary.passed, "{:?}", summary.failures);
        assert_eq!(summary.cases, 4);
        assert!((summary.false_positive_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn deploy_regression_is_identified_while_refuted_decoy_is_not_counted() {
        let case = parse_case(BUNDLED_CASES[0]).expect("parse");

        let card = replay_recorded_case(&case).expect("replay");

        assert!(card.root_cause_identified);
        assert_eq!(card.decoys_confirmed, 0);
        assert_eq!(card.policy_adjustments, 0);
    }

    #[test]
    fn supported_decoy_counts_as_false_positive() {
        let mut case = parse_case(BUNDLED_CASES[1]).expect("parse");
        let recorded = case.recorded.as_mut().expect("recorded");
        recorded.report.work_items[0].hypotheses[0].status =
            reili_core::work::HypothesisStatus::Supported;

        let card = replay_recorded_case(&case).expect("replay");

        assert!(card.root_cause_identified);
        assert_eq!(card.decoys_confirmed, 1, "CPU asserted as the cause");
    }

    #[test]
    fn decoy_mentioned_in_a_finding_counts_as_false_positive() {
        let mut case = parse_case(BUNDLED_CASES[3]).expect("parse");
        let recorded = case.recorded.as_mut().expect("recorded");
        recorded.report.work_items[0].findings[0].statement =
            "The 12:00 deploy made checkout slower".to_string();

        let card = replay_recorded_case(&case).expect("replay");

        assert_eq!(card.decoys_confirmed, 1);
    }

    #[test]
    fn live_brief_carries_metric_data_as_untrusted_context() {
        let case = parse_case(BUNDLED_CASES[0]).expect("parse");

        let rendered = super::render_metric_fixture(&case.metrics[0]);

        assert!(rendered.starts_with("query: `avg:checkout.error_rate{env:production}`"));
        assert_eq!(rendered.lines().count(), 1 + case.metrics[0].points().len());
    }

    #[test]
    fn quiet_period_produces_no_false_alarm() {
        let case = parse_case(BUNDLED_CASES[2]).expect("parse");

        let card = replay_recorded_case(&case).expect("replay");

        assert!(!card.false_alarm);
        assert!(!card.missed);
    }

    #[test]
    fn comparison_flags_regressions() {
        let (cards, expectations) = replay_all();
        let baseline = summarize(&cards, &expectations, EvaluationThresholds::default());
        let mut worse_cards = cards.clone();
        worse_cards[0].root_cause_identified = false;
        worse_cards[0].decoys_confirmed = 1;
        let candidate = summarize(&worse_cards, &expectations, EvaluationThresholds::default());

        let comparison = compare_summaries(&baseline, &candidate);

        assert_eq!(comparison.regressions.len(), 2);
        assert!(!candidate.passed);
        let markdown = render_markdown(&candidate, &worse_cards, Some(&comparison));
        assert!(markdown.contains("**Result:** FAIL"));
        assert!(markdown.contains("regression: root cause accuracy dropped"));
    }
}
