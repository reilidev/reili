//! Structured investigation vocabulary: Observation, Evidence, Hypothesis, Verification, Finding,
//! and Recommendation are kept distinct so facts, inferences, and unknowns never blur together.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::analysis::ExpectedChange;
use crate::goal::MetricAggregation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Datadog,
    Github,
    Slack,
    Canvas,
    Web,
    Jira,
    Esa,
    Metric,
    Human,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// A pointer to where something was observed. Raw logs and payloads are never stored; the
/// reference (URL, query, window) is enough for a human to reproduce the observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub id: String,
    pub source: EvidenceSource,
    #[serde(default)]
    pub url: Option<String>,
    pub description: String,
    #[serde(default, deserialize_with = "lenient_optional")]
    pub observed_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "lenient_optional")]
    pub time_range: Option<TimeRange>,
    /// Query or retrieval conditions used to obtain the evidence.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub statement: String,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Inconclusive,
    NotRun,
}

impl VerificationStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
            Self::NotRun => "not_run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMethod {
    StatisticalShift,
    ChangePointDetection,
    AnomalyDetection,
    Correlation,
    Recalculation,
    ErrorBudget,
    OutcomeComparison,
}

/// Where the numbers a verification used came from. Only data fetched by Reili itself or a pure
/// computation counts as independent of the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VerificationDataSource {
    IndependentQuery {
        query: String,
        source_url: String,
        time_range: TimeRange,
    },
    Computation,
    AgentSuppliedData {
        point_count: usize,
    },
}

impl VerificationDataSource {
    #[must_use]
    pub fn is_independent(&self) -> bool {
        matches!(self, Self::IndependentQuery { .. } | Self::Computation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub id: String,
    pub method: VerificationMethod,
    pub claim: String,
    pub status: VerificationStatus,
    pub detail: String,
    pub data_source: VerificationDataSource,
    pub performed_at: DateTime<Utc>,
    /// What additional data or method would settle an inconclusive or not-run verification.
    #[serde(default)]
    pub needed_to_settle: Option<String>,
}

impl VerificationRecord {
    /// A verification that may upgrade or refute a hypothesis.
    #[must_use]
    pub fn is_decisive(&self) -> bool {
        self.data_source.is_independent()
            && matches!(
                self.status,
                VerificationStatus::Passed | VerificationStatus::Failed
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    Proposed,
    Supported,
    Refuted,
    Inconclusive,
}

impl HypothesisStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Supported => "supported",
            Self::Refuted => "refuted",
            Self::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    pub status: HypothesisStatus,
    #[serde(default)]
    pub verification_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub statement: String,
    /// Set by Reili's verification gate, never by the model: true only when at least one
    /// independent verification passed for this finding.
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub verification_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
}

/// Metric watched after a recommendation is implemented to evaluate its real effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeMetric {
    pub query: String,
    pub expected: ExpectedChange,
    #[serde(default = "default_outcome_aggregation")]
    pub aggregation: MetricAggregation,
    /// Length of the before/after comparison windows.
    #[serde(default = "default_outcome_window_hours")]
    pub window_hours: u32,
}

fn default_outcome_aggregation() -> MetricAggregation {
    MetricAggregation::Avg
}

fn default_outcome_window_hours() -> u32 {
    72
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub action: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub expected_effect: String,
    #[serde(default)]
    pub risk: String,
    #[serde(default)]
    pub outcome_metric: Option<OutcomeMetric>,
}

/// Model-produced timestamps are best-effort: an unparseable value becomes `None` instead of
/// rejecting the whole report.
fn lenient_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[cfg(test)]
mod tests {
    use super::{EvidenceRef, VerificationDataSource};

    #[test]
    fn tolerates_malformed_evidence_timestamps() {
        let evidence: EvidenceRef = serde_json::from_str(
            r#"{"id":"E1","source":"datadog","description":"errors","observed_at":"yesterday","time_range":{"from":"x"}}"#,
        )
        .expect("deserialize");

        assert_eq!(evidence.observed_at, None);
        assert_eq!(evidence.time_range, None);
    }

    #[test]
    fn only_reili_fetched_data_is_independent() {
        assert!(VerificationDataSource::Computation.is_independent());
        assert!(!VerificationDataSource::AgentSuppliedData { point_count: 3 }.is_independent());
    }
}
