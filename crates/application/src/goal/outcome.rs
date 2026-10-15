//! Outcome evaluation: success criteria and implemented recommendations are measured directly
//! against telemetry. Effects are reported as observed associations; Reili never claims causation
//! from a before/after comparison alone.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reili_core::analysis::{ExpectedChange, ShiftCriteria, ShiftVerdict, assess_shift_around};
use reili_core::goal::{GoalDefinition, SuccessCriterion};
use reili_core::metrics::{MetricQueryInput, MetricQueryPort, aggregate_points};
use reili_core::work::{
    CriterionObservation, OutcomeAssessment, OutcomeTracking, OutcomeVerdict, StatusActor,
    WorkItemState, WorkItemStatus,
};

const MIN_POST_CHANGE_HOURS: i64 = 1;

pub async fn evaluate_success_criteria(
    goal: &GoalDefinition,
    metric_port: Option<&Arc<dyn MetricQueryPort>>,
    now: DateTime<Utc>,
) -> Vec<(String, CriterionObservation)> {
    let mut observations = Vec::new();
    for criterion in &goal.success_criteria {
        observations.push((
            criterion.id.clone(),
            evaluate_criterion(criterion, metric_port, now).await,
        ));
    }
    observations
}

async fn evaluate_criterion(
    criterion: &SuccessCriterion,
    metric_port: Option<&Arc<dyn MetricQueryPort>>,
    now: DateTime<Utc>,
) -> CriterionObservation {
    let unmeasured = |detail: &str| CriterionObservation {
        at: now,
        value: None,
        met: None,
        detail: detail.to_string(),
        source_url: None,
    };
    let Some(metric) = &criterion.metric else {
        return unmeasured("qualitative criterion; assessed by the investigation, not measured");
    };
    let Some(port) = metric_port else {
        return unmeasured("metric queries are not configured; criterion not measured");
    };
    let result = port
        .query(MetricQueryInput {
            query: metric.query.clone(),
            from: now - Duration::minutes(i64::from(metric.window_minutes)),
            to: now,
        })
        .await;
    match result {
        Ok(result) => {
            let value = aggregate_points(&result.combined_points(), metric.aggregation);
            let met = value.map(|value| criterion.comparator.is_met(value, criterion.target));
            CriterionObservation {
                at: now,
                value,
                met,
                detail: match value {
                    Some(value) => format!(
                        "{value:.6} {} {} over the last {} minutes ({})",
                        criterion.comparator.symbol(),
                        criterion.target,
                        metric.window_minutes,
                        if met == Some(true) { "met" } else { "not met" }
                    ),
                    None => "query returned no data points".to_string(),
                },
                source_url: Some(result.source_url),
            }
        }
        Err(error) => unmeasured(&format!("metric query failed: {}", error.message)),
    }
}

/// Measures an implemented recommendation's effect by comparing equal windows before and after
/// the implementation time.
pub async fn assess_outcome(
    tracking: &OutcomeTracking,
    metric_port: Option<&Arc<dyn MetricQueryPort>>,
    now: DateTime<Utc>,
) -> OutcomeAssessment {
    let window = Duration::hours(i64::from(tracking.metric.window_hours.max(1)));
    let minimum_after = (window / 2).max(Duration::hours(MIN_POST_CHANGE_HOURS));
    let pending = |detail: String| OutcomeAssessment {
        at: now,
        verdict: OutcomeVerdict::Pending,
        detail,
        source_url: None,
    };
    if now < tracking.implemented_at + minimum_after {
        return pending(format!(
            "collecting post-change data until {}",
            (tracking.implemented_at + minimum_after).to_rfc3339()
        ));
    }
    let Some(port) = metric_port else {
        return pending("metric queries are not configured".to_string());
    };
    let from = tracking.implemented_at - window;
    let to = now.min(tracking.implemented_at + window);
    let result = match port
        .query(MetricQueryInput {
            query: tracking.metric.query.clone(),
            from,
            to,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => {
            return OutcomeAssessment {
                at: now,
                verdict: OutcomeVerdict::Inconclusive,
                detail: format!("metric query failed: {}", error.message),
                source_url: None,
            };
        }
    };
    let points = result.combined_points();
    let split_ms = tracking.implemented_at.timestamp_millis();
    let criteria = ShiftCriteria::default();
    let assessment = assess_shift_around(&points, split_ms, 0, tracking.metric.expected, criteria);
    let verdict = match assessment.verdict {
        ShiftVerdict::Supported => OutcomeVerdict::Improved,
        ShiftVerdict::Contradicted => {
            let opposite = match tracking.metric.expected {
                ExpectedChange::Increase => ExpectedChange::Decrease,
                ExpectedChange::Decrease => ExpectedChange::Increase,
                other => other,
            };
            let regressed = opposite != tracking.metric.expected
                && assess_shift_around(&points, split_ms, 0, opposite, criteria).verdict
                    == ShiftVerdict::Supported;
            if regressed {
                OutcomeVerdict::Regressed
            } else {
                OutcomeVerdict::NoChange
            }
        }
        ShiftVerdict::Inconclusive => OutcomeVerdict::Inconclusive,
    };
    let detail = format!(
        "{} (before/after {}h windows around {}); observed association only — other changes in the same period are not ruled out",
        assessment.reason,
        tracking.metric.window_hours,
        tracking.implemented_at.to_rfc3339()
    );
    OutcomeAssessment {
        at: now,
        verdict,
        detail,
        source_url: Some(result.source_url),
    }
}

/// Applies an outcome assessment to a monitored work item and decides the follow-up status.
pub fn apply_outcome_assessment(
    state: &mut WorkItemState,
    assessment: OutcomeAssessment,
    now: DateTime<Utc>,
) {
    let Some(tracking) = state.outcome.as_mut() else {
        return;
    };
    let window_elapsed =
        now >= tracking.implemented_at + Duration::hours(i64::from(tracking.metric.window_hours));
    let previous = tracking.latest_verdict();
    let verdict = assessment.verdict;
    tracking.assessments.push(assessment);
    if state.status != WorkItemStatus::Monitoring {
        return;
    }
    let decision = |question: &str| reili_core::work::HumanDecisionRequest {
        question: question.to_string(),
        requested_at: now,
    };
    match verdict {
        OutcomeVerdict::Improved
            if window_elapsed || previous == Some(OutcomeVerdict::Improved) =>
        {
            state.transition(
                WorkItemStatus::Resolved,
                StatusActor::Reili,
                "outcome metric improved after the change (observed association; causality not proven)",
                now,
            );
        }
        OutcomeVerdict::Regressed => {
            state.transition(
                WorkItemStatus::NeedsHuman,
                StatusActor::Reili,
                "outcome metric regressed after the change",
                now,
            );
            state.human_decision = Some(decision(
                "The outcome metric got worse after the change. Revert, or keep and investigate?",
            ));
        }
        OutcomeVerdict::NoChange if window_elapsed => {
            state.transition(
                WorkItemStatus::NeedsHuman,
                StatusActor::Reili,
                "no measurable effect after the full observation window",
                now,
            );
            state.human_decision = Some(decision(
                "No measurable effect was observed. Close as ineffective, or keep monitoring with a different metric?",
            ));
        }
        OutcomeVerdict::Inconclusive if window_elapsed => {
            state.transition(
                WorkItemStatus::NeedsHuman,
                StatusActor::Reili,
                "effect could not be determined from the outcome metric",
                now,
            );
            state.human_decision = Some(decision(
                "The effect could not be determined from the outcome metric. Provide a better metric, or close?",
            ));
        }
        _ => {}
    }
    state.updated_at = now;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Duration;
    use reili_core::analysis::ExpectedChange;
    use reili_core::goal::{
        CriterionMetric, MetricAggregation, SuccessCriterion, TargetComparator,
    };
    use reili_core::metrics::MetricQueryPort;
    use reili_core::work::{
        OutcomeMetric, OutcomeTracking, OutcomeVerdict, WorkItemStatus,
        test_support::sample_work_item,
    };

    use super::{apply_outcome_assessment, assess_outcome, evaluate_success_criteria};
    use crate::goal::testing::{FunctionMetricSource, at};
    use crate::goal::tests_support::sample_goal;

    fn tracking(expected: ExpectedChange) -> OutcomeTracking {
        OutcomeTracking {
            metric: OutcomeMetric {
                query: "avg:checkout.error_rate{*}".to_string(),
                expected,
                aggregation: MetricAggregation::Avg,
                window_hours: 24,
            },
            implemented_at: at("2026-10-10T00:00:00Z"),
            implemented_note: "PR #42".to_string(),
            assessments: Vec::new(),
        }
    }

    fn step_source(before: f64, after: f64) -> Arc<dyn MetricQueryPort> {
        let change = at("2026-10-10T00:00:00Z");
        Arc::new(FunctionMetricSource::new(move |_, time| {
            if time < change { before } else { after }
        }))
    }

    #[tokio::test]
    async fn detects_improvement_and_resolves_after_window() {
        let port = step_source(0.05, 0.01);
        let assessment = assess_outcome(
            &tracking(ExpectedChange::Decrease),
            Some(&port),
            at("2026-10-11T06:00:00Z"),
        )
        .await;

        assert_eq!(assessment.verdict, OutcomeVerdict::Improved);
        assert!(assessment.source_url.is_some());
        assert!(assessment.detail.contains("observed association only"));

        let mut item = sample_work_item("checkout:errors");
        item.status = WorkItemStatus::Monitoring;
        item.outcome = Some(tracking(ExpectedChange::Decrease));
        apply_outcome_assessment(&mut item, assessment, at("2026-10-11T06:00:00Z"));
        assert_eq!(item.status, WorkItemStatus::Resolved);
        assert_eq!(
            item.outcome
                .as_ref()
                .map(|outcome| outcome.assessments.len()),
            Some(1)
        );
    }

    #[tokio::test]
    async fn detects_regression_and_asks_humans() {
        let port = step_source(0.01, 0.05);
        let assessment = assess_outcome(
            &tracking(ExpectedChange::Decrease),
            Some(&port),
            at("2026-10-10T18:00:00Z"),
        )
        .await;

        assert_eq!(assessment.verdict, OutcomeVerdict::Regressed);
        let mut item = sample_work_item("checkout:errors");
        item.status = WorkItemStatus::Monitoring;
        item.outcome = Some(tracking(ExpectedChange::Decrease));
        apply_outcome_assessment(&mut item, assessment, at("2026-10-10T18:00:00Z"));
        assert_eq!(item.status, WorkItemStatus::NeedsHuman);
        assert!(item.human_decision.is_some());
    }

    #[tokio::test]
    async fn reports_no_change_and_pending() {
        let port = step_source(0.02, 0.02);
        let no_change = assess_outcome(
            &tracking(ExpectedChange::Decrease),
            Some(&port),
            at("2026-10-11T06:00:00Z"),
        )
        .await;
        let pending = assess_outcome(
            &tracking(ExpectedChange::Decrease),
            Some(&port),
            at("2026-10-10T02:00:00Z"),
        )
        .await;

        assert_eq!(no_change.verdict, OutcomeVerdict::NoChange);
        assert_eq!(pending.verdict, OutcomeVerdict::Pending);
        assert!(pending.source_url.is_none());
    }

    #[tokio::test]
    async fn evaluates_success_criteria_deterministically() {
        let mut goal = sample_goal("checkout");
        goal.success_criteria = vec![
            SuccessCriterion {
                id: "error-rate".to_string(),
                description: "5xx rate below 1%".to_string(),
                metric: Some(CriterionMetric {
                    query: "avg:checkout.error_rate{*}".to_string(),
                    aggregation: MetricAggregation::Avg,
                    window_minutes: 60,
                }),
                comparator: TargetComparator::AtMost,
                target: 0.01,
            },
            SuccessCriterion {
                id: "toil".to_string(),
                description: "less manual toil".to_string(),
                metric: None,
                comparator: TargetComparator::AtMost,
                target: 0.0,
            },
        ];
        let port = step_source(0.05, 0.005);

        let observations = evaluate_success_criteria(
            &goal,
            Some(&port),
            at("2026-10-12T00:00:00Z") + Duration::hours(1),
        )
        .await;

        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].1.met, Some(true));
        assert!(observations[0].1.source_url.is_some());
        assert_eq!(observations[1].1.met, None);
    }
}
