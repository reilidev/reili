use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::Method;
use serde_json::{Value, json};
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{
    DatadogMetricQueryParams, DatadogMetricQueryPoint, DatadogMetricQueryPort,
    DatadogMetricQueryResult,
};

use super::datadog_http_client::{DatadogApiVersion, DatadogHttpClient, DatadogRequestInput};
use crate::json_utils::read_non_empty_json_string;

#[derive(Debug, Clone)]
pub struct DatadogMetricQueryAdapter {
    http_client: Arc<DatadogHttpClient>,
}

impl DatadogMetricQueryAdapter {
    pub fn new(http_client: Arc<DatadogHttpClient>) -> Self {
        Self { http_client }
    }
}

#[async_trait]
impl DatadogMetricQueryPort for DatadogMetricQueryAdapter {
    async fn query_metrics(
        &self,
        params: DatadogMetricQueryParams,
    ) -> Result<Vec<DatadogMetricQueryResult>, PortError> {
        let from_ms = to_epoch_ms(&params.from)?;
        let to_ms = to_epoch_ms(&params.to)?;
        validate_time_range(TimeRangeInput { from_ms, to_ms })?;

        let response = self
            .http_client
            .request_json(DatadogRequestInput {
                method: Method::POST,
                api_version: DatadogApiVersion::V2,
                path: "/query/timeseries".to_string(),
                query: Vec::new(),
                body: Some(json!({
                    "data": {
                        "type": "timeseries_request",
                        "attributes": {
                            "from": from_ms,
                            "to": to_ms,
                            "queries": [
                                {
                                    "data_source": "metrics",
                                    "query": params.query,
                                }
                            ],
                        },
                    },
                })),
            })
            .await?;

        let attributes = response
            .get("data")
            .and_then(|value| value.get("attributes"))
            .cloned()
            .unwrap_or(Value::Null);
        let times = extract_times(&attributes);
        let values = extract_values(&attributes);
        let series = attributes
            .get("series")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::with_capacity(series.len());
        for (series_index, series_item) in series.iter().enumerate() {
            results.push(map_series_to_metric_query_result(MapSeriesInput {
                series: series_item,
                series_index,
                times: &times,
                values: &values,
            }));
        }

        Ok(results)
    }
}

struct MapSeriesInput<'a> {
    series: &'a Value,
    series_index: usize,
    times: &'a [i64],
    values: &'a [Vec<Option<f64>>],
}

fn map_series_to_metric_query_result(input: MapSeriesInput<'_>) -> DatadogMetricQueryResult {
    let query_index = input
        .series
        .get("query_index")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let series_values = get_series_values(GetSeriesValuesInput {
        values: input.values,
        series_index: input.series_index,
        query_index,
    });
    let points = get_metric_points(MetricPointsInput {
        times: input.times,
        values: series_values,
    });
    let unit = to_unit_label(input.series.get("unit"));
    let group_tags = to_group_tags(input.series.get("group_tags"));

    DatadogMetricQueryResult {
        metric: read_metric_name(input.series),
        unit,
        group_tags,
        points,
    }
}

fn extract_times(attributes: &Value) -> Vec<i64> {
    let Some(times) = attributes.get("times").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut mapped = Vec::with_capacity(times.len());
    for time in times {
        if let Some(value) = time.as_i64() {
            mapped.push(value);
            continue;
        }

        if let Some(value) = time.as_u64().and_then(|number| i64::try_from(number).ok()) {
            mapped.push(value);
        }
    }

    mapped
}

fn extract_values(attributes: &Value) -> Vec<Vec<Option<f64>>> {
    let Some(series_values) = attributes.get("values").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut mapped = Vec::with_capacity(series_values.len());
    for series in series_values {
        let Some(points) = series.as_array() else {
            mapped.push(Vec::new());
            continue;
        };

        let mut values = Vec::with_capacity(points.len());
        for point in points {
            values.push(point.as_f64());
        }

        mapped.push(values);
    }

    mapped
}

struct GetSeriesValuesInput<'a> {
    values: &'a [Vec<Option<f64>>],
    series_index: usize,
    query_index: Option<usize>,
}

fn get_series_values(input: GetSeriesValuesInput<'_>) -> &[Option<f64>] {
    if let Some(preferred) = input.values.get(input.series_index) {
        return preferred;
    }

    if let Some(query_index) = input.query_index
        && let Some(fallback) = input.values.get(query_index)
    {
        return fallback;
    }

    if let Some(default_values) = input.values.first() {
        return default_values;
    }

    &[]
}

struct MetricPointsInput<'a> {
    times: &'a [i64],
    values: &'a [Option<f64>],
}

fn get_metric_points(input: MetricPointsInput<'_>) -> Vec<DatadogMetricQueryPoint> {
    let mut points = Vec::new();
    let max_index = input.times.len().min(input.values.len());
    if max_index == 0 {
        return points;
    }

    for index in (0..max_index).rev() {
        let value = match input.values[index] {
            Some(number) if number.is_finite() => number,
            _ => continue,
        };
        let time = input.times[index];
        points.push(DatadogMetricQueryPoint {
            time: to_iso_string(time),
            v: value,
        });
    }

    points
}

fn to_iso_string(timestamp_ms: i64) -> String {
    match DateTime::<Utc>::from_timestamp_millis(timestamp_ms) {
        Some(value) => value.to_rfc3339_opts(SecondsFormat::Millis, true),
        None => "unknown".to_string(),
    }
}

fn to_unit_label(value: Option<&Value>) -> Option<String> {
    let units = value.and_then(Value::as_array)?;
    let primary = to_single_unit_label(units.first())?;
    let per = to_single_unit_label(units.get(1));

    match per {
        Some(per_value) => Some(format!("{primary}/{per_value}")),
        None => Some(primary),
    }
}

fn to_single_unit_label(value: Option<&Value>) -> Option<String> {
    let unit = value?.as_object()?;
    read_non_empty_json_string(unit.get("short_name"))
        .or_else(|| read_non_empty_json_string(unit.get("name")))
}

fn to_group_tags(value: Option<&Value>) -> Option<Vec<String>> {
    let tags = value.and_then(Value::as_array)?;
    let mut mapped = Vec::new();
    for tag in tags {
        if let Some(text) = read_non_empty_json_string(Some(tag)) {
            mapped.push(text);
        }
    }

    if mapped.is_empty() {
        return None;
    }

    Some(mapped)
}

fn read_metric_name(value: &Value) -> Option<String> {
    read_non_empty_json_string(value.get("metric"))
        .or_else(|| read_non_empty_json_string(value.get("display_name")))
}

struct TimeRangeInput {
    from_ms: i64,
    to_ms: i64,
}

fn validate_time_range(input: TimeRangeInput) -> Result<(), PortError> {
    if input.from_ms > input.to_ms {
        return Err(PortError::new(
            "\"from\" must be earlier than or equal to \"to\"",
        ));
    }

    Ok(())
}

fn to_epoch_ms(value: &str) -> Result<i64, PortError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        PortError::new(format!(
            "Invalid time value: \"{value}\". Use an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        ))
    })?;

    Ok(parsed.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use reili_shared::ports::outbound::{DatadogMetricQueryParams, DatadogMetricQueryPort};
    use reili_shared::types::DatadogApiRetryConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DatadogMetricQueryAdapter;
    use crate::outbound::datadog::datadog_http_client::{
        DatadogHttpClient, DatadogHttpClientConfig,
    };

    #[tokio::test]
    async fn maps_timeseries_response_into_metric_query_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/query/timeseries"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "attributes": {
                        "times": [1000, 2000, 3000],
                        "values": [[1.0, null, 2.0]],
                        "series": [
                            {
                                "query_index": 0,
                                "group_tags": ["env:prod"],
                                "unit": [
                                    {"short_name": "ms"},
                                    {"name": "s"}
                                ],
                                "metric": "avg:latency"
                            }
                        ]
                    }
                }
            })))
            .mount(&server)
            .await;

        let adapter = DatadogMetricQueryAdapter::new(Arc::new(create_client(&server.uri())));
        let results = adapter
            .query_metrics(DatadogMetricQueryParams {
                query: "avg:latency{env:prod}".to_string(),
                from: "2026-03-04T09:00:00Z".to_string(),
                to: "2026-03-04T10:00:00Z".to_string(),
            })
            .await
            .expect("query metrics");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].metric.as_deref(), Some("avg:latency"));
        assert_eq!(results[0].unit.as_deref(), Some("ms/s"));
        assert_eq!(results[0].group_tags, Some(vec!["env:prod".to_string()]));
        assert_eq!(results[0].points.len(), 2);
        assert_eq!(results[0].points[0].time, "1970-01-01T00:00:03.000Z");
        assert_eq!(results[0].points[0].v, 2.0);
        assert_eq!(results[0].points[1].time, "1970-01-01T00:00:01.000Z");
        assert_eq!(results[0].points[1].v, 1.0);
    }

    #[tokio::test]
    async fn returns_error_for_invalid_time_range() {
        let adapter = DatadogMetricQueryAdapter::new(Arc::new(create_client("http://localhost")));
        let result = adapter
            .query_metrics(DatadogMetricQueryParams {
                query: "avg:latency{*}".to_string(),
                from: "2026-03-04T10:00:00Z".to_string(),
                to: "2026-03-04T09:00:00Z".to_string(),
            })
            .await;

        let error = result.expect_err("from greater than to");
        assert!(
            error
                .message
                .contains("\"from\" must be earlier than or equal to \"to\"")
        );
    }

    fn create_client(base_url: &str) -> DatadogHttpClient {
        DatadogHttpClient::new(DatadogHttpClientConfig {
            api_key: "dd-api-key".to_string(),
            app_key: "dd-app-key".to_string(),
            site: "datadoghq.com".to_string(),
            retry: DatadogApiRetryConfig {
                enabled: false,
                max_retries: 0,
                backoff_base_seconds: 2,
                backoff_multiplier: 2,
            },
            max_response_bytes: 0,
            base_url: Some(base_url.to_string()),
        })
        .expect("build datadog client")
    }
}
