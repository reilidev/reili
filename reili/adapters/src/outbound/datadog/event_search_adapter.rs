use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{
    DatadogEventSearchParams, DatadogEventSearchPort, DatadogEventSearchResult,
};
use reqwest::Method;
use serde_json::Value;

use super::datadog_http_client::{DatadogApiVersion, DatadogHttpClient, DatadogRequestInput};
use crate::json_utils::read_non_empty_json_string;

#[derive(Debug, Clone)]
pub struct DatadogEventSearchAdapter {
    http_client: Arc<DatadogHttpClient>,
}

impl DatadogEventSearchAdapter {
    pub fn new(http_client: Arc<DatadogHttpClient>) -> Self {
        Self { http_client }
    }
}

#[async_trait]
impl DatadogEventSearchPort for DatadogEventSearchAdapter {
    async fn search_events(
        &self,
        params: DatadogEventSearchParams,
    ) -> Result<Vec<DatadogEventSearchResult>, PortError> {
        let now_ms = Utc::now().timestamp_millis();
        let from_ms = resolve_time_expression_to_epoch_ms(&params.from, now_ms)?;
        let to_ms = resolve_time_expression_to_epoch_ms(&params.to, now_ms)?;
        validate_time_range(TimeRangeInput { from_ms, to_ms })?;

        let response = self
            .http_client
            .request_json(DatadogRequestInput {
                method: Method::GET,
                api_version: DatadogApiVersion::V2,
                path: "/events".to_string(),
                query: vec![
                    ("filter[query]".to_string(), params.query),
                    ("filter[from]".to_string(), to_iso_string(from_ms)),
                    ("filter[to]".to_string(), to_iso_string(to_ms)),
                    ("sort".to_string(), "-timestamp".to_string()),
                    ("page[limit]".to_string(), params.limit.to_string()),
                ],
                body: None,
            })
            .await?;

        let data = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut results = Vec::with_capacity(data.len());
        for item in data {
            results.push(map_event_search_result(&item));
        }

        Ok(results)
    }
}

fn map_event_search_result(value: &Value) -> DatadogEventSearchResult {
    let attributes = value.get("attributes");
    let nested = attributes.and_then(|field| field.get("attributes"));

    DatadogEventSearchResult {
        id: read_non_empty_json_string(value.get("id")).unwrap_or_default(),
        timestamp: read_non_empty_json_string(attributes.and_then(|field| field.get("timestamp")))
            .unwrap_or_else(|| "unknown".to_string()),
        source: read_non_empty_json_string(nested.and_then(|field| field.get("source_type_name")))
            .or_else(|| {
                read_non_empty_json_string(nested.and_then(|field| field.get("sourceTypeName")))
            }),
        status: read_non_empty_json_string(nested.and_then(|field| field.get("status"))),
        title: read_non_empty_json_string(nested.and_then(|field| field.get("title"))),
        message: read_non_empty_json_string(attributes.and_then(|field| field.get("message"))),
        tags: to_tags(attributes.and_then(|field| field.get("tags"))),
    }
}

fn to_tags(value: Option<&Value>) -> Option<Vec<String>> {
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

fn resolve_time_expression_to_epoch_ms(
    value: &str,
    reference_now_ms: i64,
) -> Result<i64, PortError> {
    if value == "now" {
        return Ok(reference_now_ms);
    }

    if value.starts_with("now+") || value.starts_with("now-") {
        return resolve_relative_time_expression(value, reference_now_ms);
    }

    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        PortError::new(format!(
            "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        ))
    })?;

    Ok(parsed.timestamp_millis())
}

fn resolve_relative_time_expression(value: &str, reference_now_ms: i64) -> Result<i64, PortError> {
    let sign = value.chars().nth(3).ok_or_else(|| {
        PortError::new(format!(
            "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        ))
    })?;
    let raw = &value[4..];
    if raw.len() < 2 {
        return Err(PortError::new(format!(
            "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        )));
    }

    let (amount_raw, unit_raw) = raw.split_at(raw.len() - 1);
    let amount = amount_raw.parse::<i64>().map_err(|_| {
        PortError::new(format!(
            "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        ))
    })?;
    let unit = unit_raw.chars().next().ok_or_else(|| {
        PortError::new(format!(
            "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
        ))
    })?;
    let offset_ms = convert_duration_to_ms(amount, unit)?;

    if sign == '-' {
        return Ok(reference_now_ms.saturating_sub(offset_ms));
    }

    if sign == '+' {
        return Ok(reference_now_ms.saturating_add(offset_ms));
    }

    Err(PortError::new(format!(
        "Invalid time expression: \"{value}\". Use date math like \"now-15m\" or an ISO 8601 string like \"2020-10-07T00:00:00+00:00\"."
    )))
}

fn convert_duration_to_ms(amount: i64, unit: char) -> Result<i64, PortError> {
    if amount < 0 {
        return Err(PortError::new("Relative time amount must be non-negative"));
    }

    let multiplier = match unit {
        's' => 1_000,
        'm' => 60 * 1_000,
        'h' => 60 * 60 * 1_000,
        'd' => 24 * 60 * 60 * 1_000,
        'w' => 7 * 24 * 60 * 60 * 1_000,
        _ => {
            return Err(PortError::new(format!(
                "Unsupported relative time unit: {unit}"
            )));
        }
    };

    Ok(amount.saturating_mul(multiplier))
}

fn to_iso_string(timestamp_ms: i64) -> String {
    match DateTime::<Utc>::from_timestamp_millis(timestamp_ms) {
        Some(value) => value.to_rfc3339_opts(SecondsFormat::Millis, true),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_shared::ports::outbound::{DatadogEventSearchParams, DatadogEventSearchPort};
    use reili_shared::types::DatadogApiRetryConfig;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DatadogEventSearchAdapter;
    use crate::outbound::datadog::datadog_http_client::{
        DatadogHttpClient, DatadogHttpClientConfig,
    };

    #[tokio::test]
    async fn maps_event_search_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {
                        "id": "event-1",
                        "attributes": {
                            "timestamp": "2026-03-04T10:00:00Z",
                            "message": "deployment completed",
                            "tags": ["service:api", "env:prod"],
                            "attributes": {
                                "source_type_name": "jenkins",
                                "status": "success",
                                "title": "Deploy finished"
                            }
                        }
                    }
                ]
            })))
            .mount(&server)
            .await;

        let adapter = DatadogEventSearchAdapter::new(Arc::new(create_client(&server.uri())));
        let results = adapter
            .search_events(DatadogEventSearchParams {
                query: "service:api".to_string(),
                from: "now-15m".to_string(),
                to: "now".to_string(),
                limit: 5,
            })
            .await
            .expect("search events");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "event-1");
        assert_eq!(results[0].timestamp, "2026-03-04T10:00:00Z");
        assert_eq!(results[0].source.as_deref(), Some("jenkins"));
        assert_eq!(results[0].status.as_deref(), Some("success"));
        assert_eq!(results[0].title.as_deref(), Some("Deploy finished"));
        assert_eq!(results[0].message.as_deref(), Some("deployment completed"));
        assert_eq!(
            results[0].tags,
            Some(vec!["service:api".to_string(), "env:prod".to_string()])
        );
    }

    #[tokio::test]
    async fn returns_error_for_invalid_time_expression() {
        let adapter = DatadogEventSearchAdapter::new(Arc::new(create_client("http://localhost")));
        let result = adapter
            .search_events(DatadogEventSearchParams {
                query: "service:api".to_string(),
                from: "not-a-time".to_string(),
                to: "now".to_string(),
                limit: 5,
            })
            .await;

        let error = result.expect_err("invalid time expression");
        assert!(error.message.contains("Invalid time expression"));
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
