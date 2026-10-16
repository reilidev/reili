//! Structured investigation vocabulary: Evidence, Hypothesis, Finding, and Recommendation are
//! kept distinct so facts, inferences, and unknowns never blur together.

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
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub statement: String,
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
    use super::EvidenceRef;

    #[test]
    fn tolerates_malformed_evidence_timestamps() {
        let evidence: EvidenceRef = serde_json::from_str(
            r#"{"id":"E1","source":"datadog","description":"errors","observed_at":"yesterday","time_range":{"from":"x"}}"#,
        )
        .expect("deserialize");

        assert_eq!(evidence.observed_at, None);
        assert_eq!(evidence.time_range, None);
    }
}
