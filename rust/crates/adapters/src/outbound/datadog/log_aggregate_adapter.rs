use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{
    DatadogLogAggregateBucket, DatadogLogAggregateParams, DatadogLogAggregatePort,
};

use super::datadog_http_client::{DatadogApiVersion, DatadogHttpClient, DatadogRequestInput};

const DEFAULT_FACET: &str = "service";

#[derive(Debug, Clone)]
pub struct DatadogLogAggregateAdapter {
    http_client: Arc<DatadogHttpClient>,
}

impl DatadogLogAggregateAdapter {
    #[must_use]
    pub fn new(http_client: Arc<DatadogHttpClient>) -> Self {
        Self { http_client }
    }
}

#[async_trait]
impl DatadogLogAggregatePort for DatadogLogAggregateAdapter {
    async fn aggregate_by_facet(
        &self,
        params: DatadogLogAggregateParams,
    ) -> Result<Vec<DatadogLogAggregateBucket>, PortError> {
        let facet = normalize_facet(&params.facet);
        let response = self
            .http_client
            .request_json(DatadogRequestInput {
                method: Method::POST,
                api_version: DatadogApiVersion::V2,
                path: "/logs/analytics/aggregate".to_string(),
                query: Vec::new(),
                body: Some(json!({
                    "filter": {
                        "query": params.query,
                        "from": params.from,
                        "to": params.to,
                    },
                    "compute": [
                        {
                            "aggregation": "count",
                        }
                    ],
                    "group_by": [
                        {
                            "facet": facet,
                            "limit": params.limit,
                            "sort": {
                                "aggregation": "count",
                                "order": "desc",
                                "type": "measure",
                            }
                        }
                    ]
                })),
            })
            .await?;

        let buckets = response
            .get("data")
            .and_then(|value| value.get("buckets"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::with_capacity(buckets.len());
        for bucket in buckets {
            let Some(key) = to_bucket_key(read_bucket_facet_value(&bucket, &facet)) else {
                continue;
            };
            let Some(count) = extract_count(bucket.get("computes")) else {
                continue;
            };

            results.push(DatadogLogAggregateBucket { key, count });
        }

        Ok(results)
    }
}

fn normalize_facet(facet: &str) -> String {
    let normalized = facet.trim();
    if normalized.is_empty() {
        return DEFAULT_FACET.to_string();
    }

    normalized.to_string()
}

fn read_bucket_facet_value<'a>(bucket: &'a Value, facet: &str) -> Option<&'a Value> {
    let values = bucket.get("by")?.as_object()?;
    for candidate in build_facet_key_candidates(facet) {
        if let Some(value) = values.get(&candidate)
            && !value.is_null()
        {
            return Some(value);
        }
    }

    None
}

fn build_facet_key_candidates(facet: &str) -> [String; 2] {
    if let Some(stripped) = facet.strip_prefix('@') {
        return [facet.to_string(), stripped.to_string()];
    }

    [facet.to_string(), format!("@{facet}")]
}

fn to_bucket_key(value: Option<&Value>) -> Option<String> {
    let key = match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(boolean)) => boolean.to_string(),
        _ => return None,
    };

    if key.is_empty() {
        return None;
    }

    Some(key)
}

fn extract_count(computes: Option<&Value>) -> Option<u64> {
    let values = computes?.as_object()?;
    if let Some(count) = values.get("count").and_then(to_count) {
        return Some(count);
    }

    values.values().find_map(to_count)
}

fn to_count(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                return Some(unsigned);
            }

            number.as_f64().and_then(f64_to_u64)
        }
        Value::String(text) => text.parse::<u64>().ok(),
        _ => None,
    }
}

fn f64_to_u64(value: f64) -> Option<u64> {
    if !value.is_finite() || value.is_sign_negative() {
        return None;
    }

    let floored = value.floor();
    if floored > u64::MAX as f64 {
        return None;
    }

    Some(floored as u64)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use sre_shared::ports::outbound::{DatadogLogAggregateParams, DatadogLogAggregatePort};
    use sre_shared::types::DatadogApiRetryConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DatadogLogAggregateAdapter;
    use crate::outbound::datadog::datadog_http_client::{
        DatadogHttpClient, DatadogHttpClientConfig,
    };

    #[tokio::test]
    async fn maps_aggregate_buckets_and_skips_invalid_entries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/logs/analytics/aggregate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {
                    "buckets": [
                        {
                            "by": {"service": "api"},
                            "computes": {"count": 12}
                        },
                        {
                            "by": {"@service": "worker"},
                            "computes": {"c0": 3}
                        },
                        {
                            "by": {"service": ""},
                            "computes": {"count": 7}
                        }
                    ]
                }
            })))
            .mount(&server)
            .await;

        let adapter = DatadogLogAggregateAdapter::new(Arc::new(create_client(&server.uri())));
        let results = adapter
            .aggregate_by_facet(DatadogLogAggregateParams {
                query: "env:prod".to_string(),
                from: "2026-03-04T09:00:00Z".to_string(),
                to: "2026-03-04T11:00:00Z".to_string(),
                facet: "service".to_string(),
                limit: 10,
            })
            .await
            .expect("aggregate logs");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key, "api");
        assert_eq!(results[0].count, 12);
        assert_eq!(results[1].key, "worker");
        assert_eq!(results[1].count, 3);
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
