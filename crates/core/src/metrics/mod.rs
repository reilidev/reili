//! Direct, read-only metric queries used for goal success criteria and outcome tracking.
//! Results come straight from the telemetry backend, never through the LLM.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::analysis::SeriesPoint;
use crate::analysis::statistics::{finite_values, mean, quantile_sorted};
use crate::error::PortError;
use crate::goal::MetricAggregation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricQueryInput {
    pub query: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSeries {
    /// Backend-provided scope/expression label for this series.
    pub label: String,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricQueryResult {
    pub query: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub series: Vec<MetricSeries>,
    /// Link a human can open to inspect the same data.
    pub source_url: String,
}

impl MetricQueryResult {
    /// Points of the single series, or the per-timestamp sum across series when the query is
    /// grouped. Callers that need per-group analysis should iterate `series` directly.
    #[must_use]
    pub fn combined_points(&self) -> Vec<SeriesPoint> {
        if self.series.len() == 1 {
            return self.series[0].points.clone();
        }
        let mut by_time = std::collections::BTreeMap::<i64, f64>::new();
        for series in &self.series {
            for point in &series.points {
                if point.value.is_finite() {
                    *by_time.entry(point.timestamp_ms).or_insert(0.0) += point.value;
                }
            }
        }
        by_time
            .into_iter()
            .map(|(timestamp_ms, value)| SeriesPoint {
                timestamp_ms,
                value,
            })
            .collect()
    }
}

#[must_use]
pub fn aggregate_points(points: &[SeriesPoint], aggregation: MetricAggregation) -> Option<f64> {
    let values = finite_values(&points.iter().map(|point| point.value).collect::<Vec<_>>());
    if values.is_empty() {
        return None;
    }
    match aggregation {
        MetricAggregation::Avg => mean(&values),
        MetricAggregation::Max => values.iter().copied().reduce(f64::max),
        MetricAggregation::Min => values.iter().copied().reduce(f64::min),
        MetricAggregation::Sum => Some(values.iter().sum()),
        MetricAggregation::Last => points
            .iter()
            .rev()
            .find(|point| point.value.is_finite())
            .map(|point| point.value),
        MetricAggregation::P95 => {
            let mut sorted = values;
            sorted.sort_by(f64::total_cmp);
            quantile_sorted(&sorted, 0.95)
        }
    }
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait MetricQueryPort: Send + Sync {
    async fn query(&self, input: MetricQueryInput) -> Result<MetricQueryResult, PortError>;
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{MetricQueryResult, MetricSeries, aggregate_points};
    use crate::analysis::SeriesPoint;
    use crate::goal::MetricAggregation;

    fn points(values: &[f64]) -> Vec<SeriesPoint> {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| SeriesPoint {
                timestamp_ms: i64::try_from(index).expect("index") * 1_000,
                value: *value,
            })
            .collect()
    }

    #[test]
    fn aggregates_points() {
        let values = points(&[1.0, 4.0, f64::NAN, 3.0]);

        assert_eq!(aggregate_points(&values, MetricAggregation::Sum), Some(8.0));
        assert_eq!(aggregate_points(&values, MetricAggregation::Max), Some(4.0));
        assert_eq!(
            aggregate_points(&values, MetricAggregation::Last),
            Some(3.0)
        );
        assert_eq!(aggregate_points(&[], MetricAggregation::Avg), None);
    }

    #[test]
    fn combines_grouped_series_by_timestamp() {
        let result = MetricQueryResult {
            query: "q".to_string(),
            from: Utc.timestamp_opt(0, 0).single().expect("time"),
            to: Utc.timestamp_opt(10, 0).single().expect("time"),
            series: vec![
                MetricSeries {
                    label: "a".to_string(),
                    points: points(&[1.0, 2.0]),
                },
                MetricSeries {
                    label: "b".to_string(),
                    points: points(&[10.0, 20.0]),
                },
            ],
            source_url: "https://app.datadoghq.com".to_string(),
        };

        let combined = result.combined_points();

        assert_eq!(combined.len(), 2);
        assert_eq!(combined[1].value, 22.0);
    }
}
