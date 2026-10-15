//! Read-only Datadog timeseries queries (`GET /api/v1/query`) used for goal success criteria and
//! outcome tracking, so the measured numbers never pass through the LLM.
//! API reference: <https://docs.datadoghq.com/api/latest/metrics/#query-timeseries-points>

use async_trait::async_trait;
use reili_core::analysis::SeriesPoint;
use reili_core::error::PortError;
use reili_core::metrics::{MetricQueryInput, MetricQueryPort, MetricQueryResult, MetricSeries};
use reili_core::secret::SecretString;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

const API_KEY_HEADER: &str = "dd-api-key";
const APPLICATION_KEY_HEADER: &str = "dd-application-key";
const MAX_SERIES: usize = 50;
const MAX_ERROR_BODY_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatadogMetricQueryConfig {
    pub api_key: SecretString,
    pub app_key: SecretString,
    pub site: String,
    /// Overrides `https://api.<site>` (tests).
    pub api_base_url: Option<String>,
}

pub struct DatadogMetricQueryAdapter {
    client: reqwest::Client,
    api_base_url: String,
    app_base_url: String,
}

impl DatadogMetricQueryAdapter {
    pub fn new(config: DatadogMetricQueryConfig) -> Result<Self, PortError> {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            (API_KEY_HEADER, &config.api_key),
            (APPLICATION_KEY_HEADER, &config.app_key),
        ] {
            let mut header = HeaderValue::from_str(value.expose()).map_err(|error| {
                PortError::invalid_input(format!("invalid Datadog credential header: {error}"))
            })?;
            header.set_sensitive(true);
            headers.insert(HeaderName::from_static(name), header);
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|error| PortError::new(format!("failed to build Datadog client: {error}")))?;
        let site = config.site.trim().trim_end_matches('/').to_string();
        Ok(Self {
            client,
            api_base_url: config
                .api_base_url
                .unwrap_or_else(|| format!("https://api.{site}")),
            app_base_url: format!("https://{}", app_host(&site)),
        })
    }

    fn explorer_url(&self, input: &MetricQueryInput) -> String {
        let query = serde_urlencoded::to_string([
            ("from_ts", input.from.timestamp_millis().to_string()),
            ("to_ts", input.to.timestamp_millis().to_string()),
            ("live", "false".to_string()),
            ("exp_metric", input.query.clone()),
        ])
        .unwrap_or_default();
        format!("{}/metric/explorer?{query}", self.app_base_url)
    }
}

/// `datadoghq.com` → `app.datadoghq.com`; regional sites such as `us3.datadoghq.com` already are
/// the app host.
fn app_host(site: &str) -> String {
    if site.split('.').count() <= 2 {
        format!("app.{site}")
    } else {
        site.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct QueryResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    series: Vec<QuerySeries>,
}

#[derive(Debug, Deserialize)]
struct QuerySeries {
    #[serde(default)]
    pointlist: Vec<(f64, Option<f64>)>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    expression: Option<String>,
}

#[async_trait]
impl MetricQueryPort for DatadogMetricQueryAdapter {
    async fn query(&self, input: MetricQueryInput) -> Result<MetricQueryResult, PortError> {
        if input.query.trim().is_empty() {
            return Err(PortError::invalid_input("metric query must not be empty"));
        }
        let response = self
            .client
            .get(format!("{}/api/v1/query", self.api_base_url))
            .query(&[
                ("from", input.from.timestamp().to_string()),
                ("to", input.to.timestamp().to_string()),
                ("query", input.query.clone()),
            ])
            .send()
            .await
            .map_err(|error| {
                PortError::connection_failed(format!("Datadog metric query failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            PortError::invalid_response(format!("failed to read Datadog response: {error}"))
        })?;
        if !status.is_success() {
            return Err(PortError::http_status(
                status.as_u16(),
                format!(
                    "Datadog metric query returned {}: {}",
                    status.as_u16(),
                    body.chars().take(MAX_ERROR_BODY_CHARS).collect::<String>()
                ),
            ));
        }
        let parsed: QueryResponse = serde_json::from_str(&body).map_err(|error| {
            PortError::invalid_response(format!("invalid Datadog query response: {error}"))
        })?;
        if parsed.status.as_deref() == Some("error") || parsed.error.is_some() {
            return Err(PortError::invalid_input(format!(
                "Datadog rejected the query: {}",
                parsed.error.unwrap_or_else(|| "unknown error".to_string())
            )));
        }
        let series = parsed
            .series
            .into_iter()
            .take(MAX_SERIES)
            .map(|series| MetricSeries {
                label: series
                    .scope
                    .or(series.expression)
                    .unwrap_or_else(|| input.query.clone()),
                points: series
                    .pointlist
                    .into_iter()
                    .filter_map(|(timestamp, value)| {
                        #[allow(clippy::cast_possible_truncation)]
                        value.map(|value| SeriesPoint {
                            timestamp_ms: timestamp as i64,
                            value,
                        })
                    })
                    .collect(),
            })
            .collect();
        Ok(MetricQueryResult {
            source_url: self.explorer_url(&input),
            query: input.query,
            from: input.from,
            to: input.to,
            series,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use reili_core::metrics::{MetricQueryInput, MetricQueryPort};
    use reili_core::secret::SecretString;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{DatadogMetricQueryAdapter, DatadogMetricQueryConfig, app_host};

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("time")
            .with_timezone(&Utc)
    }

    fn adapter(server: &MockServer) -> DatadogMetricQueryAdapter {
        DatadogMetricQueryAdapter::new(DatadogMetricQueryConfig {
            api_key: SecretString::from("api"),
            app_key: SecretString::from("app"),
            site: "datadoghq.com".to_string(),
            api_base_url: Some(server.uri()),
        })
        .expect("adapter")
    }

    fn input() -> MetricQueryInput {
        MetricQueryInput {
            query: "avg:checkout.error_rate{env:production}".to_string(),
            from: at("2026-10-15T00:00:00Z"),
            to: at("2026-10-15T01:00:00Z"),
        }
    }

    #[tokio::test]
    async fn queries_timeseries_with_credentials_and_skips_nulls() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query"))
            .and(header("dd-api-key", "api"))
            .and(header("dd-application-key", "app"))
            .and(query_param("from", "1792022400"))
            .and(query_param("query", "avg:checkout.error_rate{env:production}"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "ok",
                "series": [{
                    "scope": "env:production",
                    "pointlist": [[1792022400000.0, 0.01], [1792022460000.0, null], [1792022520000.0, 0.02]]
                }]
            })))
            .mount(&server)
            .await;

        let result = adapter(&server).query(input()).await.expect("query");

        assert_eq!(result.series.len(), 1);
        assert_eq!(result.series[0].label, "env:production");
        assert_eq!(result.series[0].points.len(), 2);
        assert_eq!(result.series[0].points[1].timestamp_ms, 1_792_022_520_000);
        assert!(
            result
                .source_url
                .starts_with("https://app.datadoghq.com/metric/explorer?")
        );
    }

    #[tokio::test]
    async fn surfaces_query_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "error",
                "error": "Error parsing query"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query"))
            .and(query_param("query", "forbidden"))
            .respond_with(
                ResponseTemplate::new(403).set_body_string("{\"errors\":[\"Forbidden\"]}"),
            )
            .with_priority(1)
            .mount(&server)
            .await;

        let parse_error = adapter(&server).query(input()).await.expect_err("error");
        let mut forbidden = input();
        forbidden.query = "forbidden".to_string();
        let http_error = adapter(&server).query(forbidden).await.expect_err("error");

        assert!(parse_error.message.contains("Error parsing query"));
        assert_eq!(http_error.status_code(), Some(403));
    }

    #[test]
    fn derives_app_host_from_site() {
        assert_eq!(app_host("datadoghq.com"), "app.datadoghq.com");
        assert_eq!(app_host("datadoghq.eu"), "app.datadoghq.eu");
        assert_eq!(app_host("us3.datadoghq.com"), "us3.datadoghq.com");
    }
}
