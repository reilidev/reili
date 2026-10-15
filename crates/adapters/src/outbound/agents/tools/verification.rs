//! Deterministic analysis and verification tools ("outside judge"). Numbers are recomputed and
//! metric claims are tested on data Reili fetches itself; every verdict is recorded in the run's
//! verification ledger, which is the only source the verification gate trusts.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reili_core::analysis::{
    ExpectedChange, SeriesPoint, ShiftCriteria, ShiftVerdict, assess_shift_around, compare_around,
    detect_anomalies, detect_change_points, error_budget, evaluate_expression,
    strongest_lagged_correlation, summarize, trend,
};
use reili_core::error::PortError;
use reili_core::investigation::{NewVerification, VerificationLedger};
use reili_core::metrics::{MetricQueryInput, MetricQueryPort, MetricQueryResult};
use reili_core::work::{
    TimeRange, VerificationDataSource, VerificationMethod, VerificationRecord, VerificationStatus,
};
use rig::tool::{Tool, ToolContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::support::json::to_json_string;

const MAX_QUERY_WINDOW_DAYS: i64 = 31;
const MAX_AGENT_POINTS: usize = 5_000;
const MAX_CHANGE_POINTS: usize = 3;
const ANOMALY_THRESHOLD: f64 = 3.5;
const DEFAULT_RELATIVE_TOLERANCE: f64 = 0.005;

/// Shared state for all verification tools in one run.
#[derive(Clone)]
pub struct VerificationToolset {
    ledger: VerificationLedger,
    metric_port: Option<Arc<dyn MetricQueryPort>>,
}

impl VerificationToolset {
    #[must_use]
    pub fn new(ledger: VerificationLedger, metric_port: Option<Arc<dyn MetricQueryPort>>) -> Self {
        Self {
            ledger,
            metric_port,
        }
    }

    #[must_use]
    pub fn ledger(&self) -> VerificationLedger {
        self.ledger.clone()
    }

    #[must_use]
    pub fn has_metrics(&self) -> bool {
        self.metric_port.is_some()
    }

    fn record(&self, verification: NewVerification) -> VerificationRecord {
        self.ledger.record(verification, Utc::now())
    }

    async fn fetch(
        &self,
        query: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<MetricQueryResult, PortError> {
        let port = self
            .metric_port
            .as_ref()
            .ok_or_else(|| PortError::invalid_input("metric queries are not configured"))?;
        if to <= from {
            return Err(PortError::invalid_input("`to` must be after `from`"));
        }
        if to - from > Duration::days(MAX_QUERY_WINDOW_DAYS) {
            return Err(PortError::invalid_input(format!(
                "query window must not exceed {MAX_QUERY_WINDOW_DAYS} days"
            )));
        }
        port.query(MetricQueryInput {
            query: query.trim().to_string(),
            from,
            to,
        })
        .await
    }
}

fn parse_time(field: &str, value: &str) -> Result<DateTime<Utc>, PortError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| {
            PortError::invalid_input(format!(
                "`{field}` must be an RFC 3339 timestamp such as 2026-10-15T09:00:00Z: {error}"
            ))
        })
}

fn independent_source(result: &MetricQueryResult) -> VerificationDataSource {
    VerificationDataSource::IndependentQuery {
        query: result.query.clone(),
        source_url: result.source_url.clone(),
        time_range: TimeRange {
            from: result.from,
            to: result.to,
        },
    }
}

fn verdict_status(verdict: ShiftVerdict) -> VerificationStatus {
    match verdict {
        ShiftVerdict::Supported => VerificationStatus::Passed,
        ShiftVerdict::Contradicted => VerificationStatus::Failed,
        ShiftVerdict::Inconclusive => VerificationStatus::Inconclusive,
    }
}

fn record_json(record: &VerificationRecord) -> Value {
    json!({
        "verification_id": record.id,
        "status": record.status.as_str(),
        "independent": record.data_source.is_independent(),
        "detail": record.detail,
        "needed_to_settle": record.needed_to_settle,
    })
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct CalculateTool {
    toolset: VerificationToolset,
}

impl CalculateTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculateArgs {
    pub expression: String,
    #[serde(default)]
    pub claim: Option<String>,
    #[serde(default)]
    pub claimed_result: Option<f64>,
    #[serde(default)]
    pub relative_tolerance: Option<f64>,
}

impl Tool for CalculateTool {
    const NAME: &'static str = "calculate";

    type Error = PortError;
    type Args = CalculateArgs;
    type Output = String;

    fn description(&self) -> String {
        "Evaluate an arithmetic expression exactly (+ - * / % ^, parentheses, abs, sqrt, ln, log10, exp, min, max, sum, avg, round, floor, ceil, pow). Use it for every number you report (rates, percentages, budgets). Pass `claimed_result` to verify a number you or a sub-agent stated; the result is recorded as a verification.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expression": { "type": "string", "description": "e.g. 452 / 120000 * 100" },
                "claim": { "type": "string", "description": "What the number means, e.g. 'checkout 5xx rate is 0.38%'." },
                "claimed_result": { "type": "number", "description": "Value to verify against the computed result." },
                "relative_tolerance": { "type": "number", "description": "Allowed relative difference (default 0.005)." }
            },
            "required": ["expression"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let value = evaluate_expression(&args.expression)
            .map_err(|error| PortError::invalid_input(error.to_string()))?;
        let Some(claimed) = args.claimed_result else {
            return to_json_string(&json!({ "result": value }));
        };
        let tolerance = args
            .relative_tolerance
            .unwrap_or(DEFAULT_RELATIVE_TOLERANCE)
            .clamp(0.0, 0.5);
        let matches = (value - claimed).abs() <= value.abs().max(claimed.abs()) * tolerance
            || (value - claimed).abs() < 1e-12;
        let record = self.toolset.record(NewVerification {
            method: VerificationMethod::Recalculation,
            claim: args
                .claim
                .unwrap_or_else(|| format!("{} = {claimed}", args.expression)),
            status: if matches {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            },
            detail: format!(
                "{} = {value} (claimed {claimed}, tolerance {tolerance})",
                args.expression
            ),
            data_source: VerificationDataSource::Computation,
            needed_to_settle: None,
        });
        to_json_string(&json!({ "result": value, "verification": record_json(&record) }))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct VerifyMetricChangeTool {
    toolset: VerificationToolset,
}

impl VerifyMetricChangeTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyMetricChangeArgs {
    pub claim: String,
    pub query: String,
    pub from: String,
    pub to: String,
    pub change_time: String,
    pub expected: ExpectedChange,
    #[serde(default)]
    pub exclude_minutes: Option<u32>,
    #[serde(default)]
    pub min_relative_change: Option<f64>,
}

impl Tool for VerifyMetricChangeTool {
    const NAME: &'static str = "verify_metric_change";

    type Error = PortError;
    type Args = VerifyMetricChangeArgs;
    type Output = String;

    fn description(&self) -> String {
        "Independently test a claim that a metric changed at a specific time (deploy, config change, incident start). Reili fetches the Datadog metric itself and compares the windows before and after `change_time` with Welch's t-test and Mann-Whitney U, and checks whether a pre-existing trend explains the change. The verdict (passed/failed/inconclusive) is recorded as a verification you must cite by id.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "claim": { "type": "string", "description": "The claim under test, e.g. 'checkout 5xx rate increased after deploy 42'." },
                "query": { "type": "string", "description": "Datadog metric query, e.g. avg:trace.http.request.errors{service:checkout-api}.as_count()" },
                "from": { "type": "string", "description": "RFC 3339 start of the before window." },
                "to": { "type": "string", "description": "RFC 3339 end of the after window." },
                "change_time": { "type": "string", "description": "RFC 3339 time of the suspected change." },
                "expected": { "type": "string", "enum": ["increase", "decrease", "any_change", "no_change"] },
                "exclude_minutes": { "type": "integer", "description": "Ignore this many minutes on each side of change_time (rollout noise)." },
                "min_relative_change": { "type": "number", "description": "Smallest material relative change (default 0.1 = 10%)." }
            },
            "required": ["claim", "query", "from", "to", "change_time", "expected"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let from = parse_time("from", &args.from)?;
        let to = parse_time("to", &args.to)?;
        let change_time = parse_time("change_time", &args.change_time)?;
        if change_time <= from || change_time >= to {
            return Err(PortError::invalid_input(
                "`change_time` must lie between `from` and `to`",
            ));
        }
        let result = self.toolset.fetch(&args.query, from, to).await?;
        let points = result.combined_points();
        let exclude_ms = i64::from(args.exclude_minutes.unwrap_or(0)) * 60_000;
        let criteria = ShiftCriteria {
            min_relative_change: args
                .min_relative_change
                .unwrap_or(ShiftCriteria::default().min_relative_change)
                .clamp(0.0, 10.0),
            ..ShiftCriteria::default()
        };
        let split_ms = change_time.timestamp_millis();
        let assessment =
            assess_shift_around(&points, split_ms, exclude_ms, args.expected, criteria);
        let comparison = compare_around(&points, split_ms, exclude_ms);
        let status = verdict_status(assessment.verdict);
        let record = self.toolset.record(NewVerification {
            method: VerificationMethod::StatisticalShift,
            claim: args.claim,
            status,
            detail: assessment.reason.clone(),
            data_source: independent_source(&result),
            needed_to_settle: (status == VerificationStatus::Inconclusive).then(|| {
                "longer windows, a less noisy metric, or a direct signal (logs/traces) tied to the change".to_string()
            }),
        });
        to_json_string(&json!({
            "verification": record_json(&record),
            "before": comparison.before,
            "after": comparison.after,
            "source_url": result.source_url,
            "series_count": result.series.len(),
            "point_count": points.len(),
            "note": "A passed verification shows the metric changed at that time; it does not by itself prove the change caused it. Look for a mechanism (logs, traces, code) before calling it a root cause."
        }))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct AnalyzeMetricTool {
    toolset: VerificationToolset,
}

impl AnalyzeMetricTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeMetricArgs {
    pub query: String,
    pub from: String,
    pub to: String,
}

fn describe_series(points: &[SeriesPoint]) -> Value {
    let values: Vec<f64> = points.iter().map(|point| point.value).collect();
    let change_points = detect_change_points(points, MAX_CHANGE_POINTS, 5, 0.01);
    let anomalies = detect_anomalies(points, ANOMALY_THRESHOLD);
    let to_iso = |ms: i64| {
        DateTime::<Utc>::from_timestamp_millis(ms)
            .map_or_else(String::new, |time| time.to_rfc3339())
    };
    json!({
        "summary": summarize(&values),
        "trend_per_hour": trend(points).map(|trend| json!({ "slope": trend.slope, "r_squared": trend.r_squared })),
        "change_points": change_points.iter().map(|change| json!({
            "at": to_iso(change.timestamp_ms),
            "mean_before": change.mean_before,
            "mean_after": change.mean_after,
            "adjusted_p_value": change.adjusted_p_value,
        })).collect::<Vec<_>>(),
        "anomalies": anomalies.iter().take(20).map(|anomaly| json!({
            "at": to_iso(anomaly.timestamp_ms),
            "value": anomaly.value,
            "robust_z": anomaly.robust_z_score,
        })).collect::<Vec<_>>(),
        "anomaly_count": anomalies.len(),
    })
}

impl Tool for AnalyzeMetricTool {
    const NAME: &'static str = "analyze_metric";

    type Error = PortError;
    type Args = AnalyzeMetricArgs;
    type Output = String;

    fn description(&self) -> String {
        "Fetch a Datadog metric directly and describe it deterministically: summary statistics, hourly trend, change points (binary segmentation, Bonferroni-adjusted), and robust outliers. Use it to find when something changed before forming hypotheses. Exploration only; use verify_metric_change to test a specific claim.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Datadog metric query." },
                "from": { "type": "string", "description": "RFC 3339 start." },
                "to": { "type": "string", "description": "RFC 3339 end." }
            },
            "required": ["query", "from", "to"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let from = parse_time("from", &args.from)?;
        let to = parse_time("to", &args.to)?;
        let result = self.toolset.fetch(&args.query, from, to).await?;
        let per_series: Vec<Value> = result
            .series
            .iter()
            .take(10)
            .map(|series| json!({ "label": series.label, "analysis": describe_series(&series.points) }))
            .collect();
        to_json_string(&json!({
            "source_url": result.source_url,
            "combined": describe_series(&result.combined_points()),
            "series": per_series,
        }))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct CorrelateMetricsTool {
    toolset: VerificationToolset,
}

impl CorrelateMetricsTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelateMetricsArgs {
    pub claim: String,
    pub left_query: String,
    pub right_query: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub max_lag_steps: Option<i64>,
}

impl Tool for CorrelateMetricsTool {
    const NAME: &'static str = "correlate_metrics";

    type Error = PortError;
    type Args = CorrelateMetricsArgs;
    type Output = String;

    fn description(&self) -> String {
        "Fetch two Datadog metrics and compute the strongest lagged Pearson/Spearman correlation. Correlation is recorded as an inconclusive verification: it can point to candidates but never confirms causation.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "claim": { "type": "string" },
                "left_query": { "type": "string" },
                "right_query": { "type": "string" },
                "from": { "type": "string" },
                "to": { "type": "string" },
                "max_lag_steps": { "type": "integer", "description": "Lags to scan in both directions (default 6)." }
            },
            "required": ["claim", "left_query", "right_query", "from", "to"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let from = parse_time("from", &args.from)?;
        let to = parse_time("to", &args.to)?;
        let left = self.toolset.fetch(&args.left_query, from, to).await?;
        let right = self.toolset.fetch(&args.right_query, from, to).await?;
        let correlation = strongest_lagged_correlation(
            &left.combined_points(),
            &right.combined_points(),
            args.max_lag_steps.unwrap_or(6).clamp(0, 60),
        );
        let detail = correlation.as_ref().map_or_else(
            || "not enough aligned points to correlate".to_string(),
            |value| {
                format!(
                    "pearson r={:.3} (spearman {}) at lag {} steps over {} points, p={}; association only, not causation",
                    value.pearson,
                    value
                        .spearman
                        .map_or_else(|| "n/a".to_string(), |rho| format!("{rho:.3}")),
                    value.lag_steps,
                    value.sample_count,
                    value
                        .p_value
                        .map_or_else(|| "n/a".to_string(), |p| format!("{p:.2e}"))
                )
            },
        );
        let record = self.toolset.record(NewVerification {
            method: VerificationMethod::Correlation,
            claim: args.claim,
            status: VerificationStatus::Inconclusive,
            detail: detail.clone(),
            data_source: independent_source(&left),
            needed_to_settle: Some(
                "a mechanism linking the two signals (logs, traces, code) or a controlled before/after comparison".to_string(),
            ),
        });
        to_json_string(&json!({
            "verification": record_json(&record),
            "correlation": correlation,
            "left_source_url": left.source_url,
            "right_source_url": right.source_url,
        }))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct EvaluateErrorBudgetTool {
    toolset: VerificationToolset,
}

impl EvaluateErrorBudgetTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateErrorBudgetArgs {
    pub slo_target: f64,
    pub good_events_query: String,
    pub total_events_query: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub claim_objective_met: Option<bool>,
}

impl Tool for EvaluateErrorBudgetTool {
    const NAME: &'static str = "evaluate_error_budget";

    type Error = PortError;
    type Args = EvaluateErrorBudgetArgs;
    type Output = String;

    fn description(&self) -> String {
        "Compute SLO attainment, error budget consumption, and burn rate from good/total event count metrics fetched directly from Datadog (use .as_count() queries). Optionally verify a claim that the objective was or was not met.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "slo_target": { "type": "number", "description": "e.g. 0.999" },
                "good_events_query": { "type": "string" },
                "total_events_query": { "type": "string" },
                "from": { "type": "string" },
                "to": { "type": "string" },
                "claim_objective_met": { "type": "boolean" }
            },
            "required": ["slo_target", "good_events_query", "total_events_query", "from", "to"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        let from = parse_time("from", &args.from)?;
        let to = parse_time("to", &args.to)?;
        let good = self
            .toolset
            .fetch(&args.good_events_query, from, to)
            .await?;
        let total = self
            .toolset
            .fetch(&args.total_events_query, from, to)
            .await?;
        let sum = |result: &MetricQueryResult| {
            result
                .combined_points()
                .iter()
                .map(|point| point.value)
                .filter(|value| value.is_finite())
                .sum::<f64>()
        };
        let report = error_budget(args.slo_target, sum(&good), sum(&total)).ok_or_else(|| {
            PortError::invalid_input(
                "cannot compute an error budget: target must be in (0,1) and 0 <= good <= total with total > 0",
            )
        })?;
        let verification = args.claim_objective_met.map(|claimed| {
            let record = self.toolset.record(NewVerification {
                method: VerificationMethod::ErrorBudget,
                claim: format!(
                    "SLO {} {} between {} and {}",
                    args.slo_target,
                    if claimed { "was met" } else { "was missed" },
                    from.to_rfc3339(),
                    to.to_rfc3339()
                ),
                status: if claimed == report.objective_met {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
                detail: format!(
                    "achieved {:.5} vs target {}; budget consumed {:.1}%; burn rate {:.2}",
                    report.achieved,
                    report.target,
                    report.budget_consumed * 100.0,
                    report.burn_rate
                ),
                data_source: independent_source(&total),
                needed_to_settle: None,
            });
            record_json(&record)
        });
        to_json_string(&json!({
            "error_budget": report,
            "verification": verification,
            "good_source_url": good.source_url,
            "total_source_url": total.source_url,
        }))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct AnalyzeSeriesTool {
    toolset: VerificationToolset,
}

impl AnalyzeSeriesTool {
    #[must_use]
    pub fn new(toolset: VerificationToolset) -> Self {
        Self { toolset }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPoint {
    pub timestamp: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeSeriesArgs {
    #[serde(default)]
    pub claim: Option<String>,
    pub source_description: String,
    pub points: Vec<AgentPoint>,
    #[serde(default)]
    pub change_time: Option<String>,
    #[serde(default)]
    pub expected: Option<ExpectedChange>,
}

impl Tool for AnalyzeSeriesTool {
    const NAME: &'static str = "analyze_series";

    type Error = PortError;
    type Args = AnalyzeSeriesArgs;
    type Output = String;

    fn description(&self) -> String {
        "Run the same statistics on a time series you supply (e.g. log counts per interval from another tool). Because the data passed through the model, results are recorded as non-independent and cannot by themselves mark a hypothesis supported; prefer verify_metric_change when a metric exists.".to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "claim": { "type": "string" },
                "source_description": { "type": "string", "description": "Where the numbers came from (tool and query)." },
                "points": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "timestamp": { "type": "string" }, "value": { "type": "number" } },
                        "required": ["timestamp", "value"]
                    }
                },
                "change_time": { "type": "string" },
                "expected": { "type": "string", "enum": ["increase", "decrease", "any_change", "no_change"] }
            },
            "required": ["source_description", "points"]
        })
    }

    async fn call(
        &self,
        _context: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        if args.points.len() > MAX_AGENT_POINTS {
            return Err(PortError::invalid_input(format!(
                "at most {MAX_AGENT_POINTS} points are accepted"
            )));
        }
        let mut points = args
            .points
            .iter()
            .map(|point| {
                parse_time("timestamp", &point.timestamp).map(|time| SeriesPoint {
                    timestamp_ms: time.timestamp_millis(),
                    value: point.value,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        points.sort_by_key(|point| point.timestamp_ms);
        let mut output = json!({ "analysis": describe_series(&points) });
        if let (Some(change_time), Some(expected)) = (&args.change_time, args.expected) {
            let split = parse_time("change_time", change_time)?;
            let assessment = assess_shift_around(
                &points,
                split.timestamp_millis(),
                0,
                expected,
                ShiftCriteria::default(),
            );
            let record = self.toolset.record(NewVerification {
                method: VerificationMethod::StatisticalShift,
                claim: args
                    .claim
                    .unwrap_or_else(|| format!("{expected:?} at {change_time}")),
                status: verdict_status(assessment.verdict),
                detail: format!(
                    "{} (data supplied by the agent from: {})",
                    assessment.reason, args.source_description
                ),
                data_source: VerificationDataSource::AgentSuppliedData {
                    point_count: points.len(),
                },
                needed_to_settle: Some(
                    "re-test with verify_metric_change on a metric Reili can fetch directly"
                        .to_string(),
                ),
            });
            output["verification"] = record_json(&record);
        }
        to_json_string(&output)
    }
}

/// Catalog names of the verification tools, in display order.
#[must_use]
pub fn verification_tool_summaries(metrics_available: bool) -> Vec<(&'static str, &'static str)> {
    let mut entries = vec![
        (
            CalculateTool::NAME,
            "Exact arithmetic; verify any stated number (recorded verification).",
        ),
        (
            AnalyzeSeriesTool::NAME,
            "Statistics/change points on agent-supplied series (non-independent).",
        ),
    ];
    if metrics_available {
        entries.extend([
            (VerifyMetricChangeTool::NAME, "Independently test 'metric X changed at time T' with before/after statistics (recorded verification)."),
            (AnalyzeMetricTool::NAME, "Fetch a Datadog metric and find change points, trend, and outliers."),
            (CorrelateMetricsTool::NAME, "Lagged correlation between two metrics (association only)."),
            (EvaluateErrorBudgetTool::NAME, "SLO attainment, error budget, and burn rate from event counts."),
        ]);
    }
    entries
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};
    use reili_core::analysis::SeriesPoint;
    use reili_core::error::PortError;
    use reili_core::investigation::VerificationLedger;
    use reili_core::metrics::{MetricQueryInput, MetricQueryPort, MetricQueryResult, MetricSeries};
    use reili_core::work::VerificationStatus;
    use rig::tool::{Tool, ToolContext};
    use serde_json::Value;

    use super::{
        AgentPoint, AnalyzeMetricArgs, AnalyzeMetricTool, AnalyzeSeriesArgs, AnalyzeSeriesTool,
        CalculateArgs, CalculateTool, CorrelateMetricsArgs, CorrelateMetricsTool,
        EvaluateErrorBudgetArgs, EvaluateErrorBudgetTool, VerificationToolset,
        VerifyMetricChangeArgs, VerifyMetricChangeTool,
    };

    struct StepMetrics;

    #[async_trait]
    impl MetricQueryPort for StepMetrics {
        async fn query(&self, input: MetricQueryInput) -> Result<MetricQueryResult, PortError> {
            let split = DateTime::parse_from_rfc3339("2026-10-15T06:00:00Z")
                .expect("time")
                .with_timezone(&Utc);
            let mut points = Vec::new();
            let mut cursor = input.from;
            let mut index = 0;
            while cursor < input.to {
                let base = match input.query.as_str() {
                    "errors" => {
                        if cursor < split {
                            10.0
                        } else {
                            40.0
                        }
                    }
                    "good" => 995.0,
                    "total" => 1_000.0,
                    _ => 100.0,
                };
                points.push(SeriesPoint {
                    timestamp_ms: cursor.timestamp_millis(),
                    value: base * (1.0 + [0.0, 0.03, -0.03][index % 3]),
                });
                cursor += Duration::minutes(10);
                index += 1;
            }
            Ok(MetricQueryResult {
                query: input.query.clone(),
                from: input.from,
                to: input.to,
                series: vec![MetricSeries {
                    label: input.query,
                    points,
                }],
                source_url: "https://app.datadoghq.com/metric/explorer".to_string(),
            })
        }
    }

    fn toolset() -> (VerificationToolset, VerificationLedger) {
        let ledger = VerificationLedger::new();
        (
            VerificationToolset::new(ledger.clone(), Some(Arc::new(StepMetrics))),
            ledger,
        )
    }

    fn parse(output: &str) -> Value {
        serde_json::from_str(output).expect("json output")
    }

    #[tokio::test]
    async fn calculate_verifies_claimed_numbers() {
        let (toolset, ledger) = toolset();
        let tool = CalculateTool::new(toolset);

        let ok = tool
            .call(
                &mut ToolContext::new(),
                CalculateArgs {
                    expression: "452 / 120000 * 100".to_string(),
                    claim: Some("5xx rate is 0.38%".to_string()),
                    claimed_result: Some(0.3767),
                    relative_tolerance: Some(0.01),
                },
            )
            .await
            .expect("calculate");
        let wrong = tool
            .call(
                &mut ToolContext::new(),
                CalculateArgs {
                    expression: "452 / 120000 * 100".to_string(),
                    claim: None,
                    claimed_result: Some(3.8),
                    relative_tolerance: None,
                },
            )
            .await
            .expect("calculate");

        assert_eq!(parse(&ok)["verification"]["status"], "passed");
        assert_eq!(parse(&wrong)["verification"]["status"], "failed");
        let records = ledger.records();
        assert_eq!(records.len(), 2);
        assert!(records[0].data_source.is_independent());
    }

    #[tokio::test]
    async fn verify_metric_change_records_independent_verdicts() {
        let (toolset, ledger) = toolset();
        let tool = VerifyMetricChangeTool::new(toolset);
        let args = |query: &str, expected| VerifyMetricChangeArgs {
            claim: format!("{query} changed at 06:00"),
            query: query.to_string(),
            from: "2026-10-15T00:00:00Z".to_string(),
            to: "2026-10-15T12:00:00Z".to_string(),
            change_time: "2026-10-15T06:00:00Z".to_string(),
            expected,
            exclude_minutes: Some(10),
            min_relative_change: None,
        };

        let errors = tool
            .call(
                &mut ToolContext::new(),
                args("errors", reili_core::analysis::ExpectedChange::Increase),
            )
            .await
            .expect("verify");
        let traffic = tool
            .call(
                &mut ToolContext::new(),
                args("traffic", reili_core::analysis::ExpectedChange::Increase),
            )
            .await
            .expect("verify");

        assert_eq!(parse(&errors)["verification"]["verification_id"], "V1");
        assert_eq!(parse(&errors)["verification"]["status"], "passed");
        assert_eq!(parse(&traffic)["verification"]["status"], "failed");
        assert!(ledger.records().iter().all(|record| record.is_decisive()));
    }

    #[tokio::test]
    async fn verify_metric_change_rejects_invalid_windows() {
        let (toolset, _) = toolset();
        let tool = VerifyMetricChangeTool::new(toolset);

        let error = tool
            .call(
                &mut ToolContext::new(),
                VerifyMetricChangeArgs {
                    claim: "x".to_string(),
                    query: "errors".to_string(),
                    from: "2026-10-15T00:00:00Z".to_string(),
                    to: "2026-10-15T12:00:00Z".to_string(),
                    change_time: "2026-10-16T00:00:00Z".to_string(),
                    expected: reili_core::analysis::ExpectedChange::Increase,
                    exclude_minutes: None,
                    min_relative_change: None,
                },
            )
            .await
            .expect_err("invalid change time");

        assert!(error.is_invalid_input());
    }

    #[tokio::test]
    async fn analyze_metric_finds_change_point() {
        let (toolset, ledger) = toolset();
        let tool = AnalyzeMetricTool::new(toolset);

        let output = tool
            .call(
                &mut ToolContext::new(),
                AnalyzeMetricArgs {
                    query: "errors".to_string(),
                    from: "2026-10-15T00:00:00Z".to_string(),
                    to: "2026-10-15T12:00:00Z".to_string(),
                },
            )
            .await
            .expect("analyze");

        let change_points = parse(&output)["combined"]["change_points"].clone();
        assert_eq!(change_points.as_array().expect("array").len(), 1);
        assert!(
            change_points[0]["at"]
                .as_str()
                .expect("at")
                .starts_with("2026-10-15T06:00")
        );
        assert!(
            ledger.records().is_empty(),
            "exploration does not record verifications"
        );
    }

    #[tokio::test]
    async fn correlation_is_never_decisive() {
        let (toolset, ledger) = toolset();
        let tool = CorrelateMetricsTool::new(toolset);

        tool.call(
            &mut ToolContext::new(),
            CorrelateMetricsArgs {
                claim: "errors follow traffic".to_string(),
                left_query: "errors".to_string(),
                right_query: "traffic".to_string(),
                from: "2026-10-15T00:00:00Z".to_string(),
                to: "2026-10-15T12:00:00Z".to_string(),
                max_lag_steps: Some(2),
            },
        )
        .await
        .expect("correlate");

        let record = &ledger.records()[0];
        assert_eq!(record.status, VerificationStatus::Inconclusive);
        assert!(!record.is_decisive());
    }

    #[tokio::test]
    async fn error_budget_verifies_objective_claims() {
        let (toolset, ledger) = toolset();
        let tool = EvaluateErrorBudgetTool::new(toolset);

        let output = tool
            .call(
                &mut ToolContext::new(),
                EvaluateErrorBudgetArgs {
                    slo_target: 0.999,
                    good_events_query: "good".to_string(),
                    total_events_query: "total".to_string(),
                    from: "2026-10-15T00:00:00Z".to_string(),
                    to: "2026-10-15T01:00:00Z".to_string(),
                    claim_objective_met: Some(true),
                },
            )
            .await
            .expect("budget");

        assert_eq!(parse(&output)["error_budget"]["objective_met"], false);
        assert_eq!(ledger.records()[0].status, VerificationStatus::Failed);
    }

    #[tokio::test]
    async fn agent_supplied_series_is_recorded_as_non_independent() {
        let (toolset, ledger) = toolset();
        let tool = AnalyzeSeriesTool::new(toolset);
        let points = (0..20)
            .map(|index| AgentPoint {
                timestamp: format!("2026-10-15T{:02}:00:00Z", index),
                value: if index < 10 {
                    5.0 + f64::from(index % 2)
                } else {
                    50.0 + f64::from(index % 2)
                },
            })
            .collect();

        tool.call(
            &mut ToolContext::new(),
            AnalyzeSeriesArgs {
                claim: Some("log errors jumped at 10:00".to_string()),
                source_description: "search_datadog_logs count by hour".to_string(),
                points,
                change_time: Some("2026-10-15T10:00:00Z".to_string()),
                expected: Some(reili_core::analysis::ExpectedChange::Increase),
            },
        )
        .await
        .expect("analyze series");

        let record = &ledger.records()[0];
        assert_eq!(record.status, VerificationStatus::Passed);
        assert!(
            !record.is_decisive(),
            "model-relayed data never decides on its own"
        );
    }
}
