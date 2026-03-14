use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::error::PortError;
use reili_shared::monitoring::datadog::{
    DatadogLogSearchParams, DatadogLogSearchPort, DatadogLogSearchResult,
};
use reqwest::Method;
use serde_json::{Value, json};

use super::datadog_http_client::{DatadogApiVersion, DatadogHttpClient, DatadogRequestInput};

#[derive(Debug, Clone)]
pub struct DatadogLogSearchAdapter {
    http_client: Arc<DatadogHttpClient>,
}

impl DatadogLogSearchAdapter {
    pub fn new(http_client: Arc<DatadogHttpClient>) -> Self {
        Self { http_client }
    }
}

#[async_trait]
impl DatadogLogSearchPort for DatadogLogSearchAdapter {
    async fn search_logs(
        &self,
        params: DatadogLogSearchParams,
    ) -> Result<Vec<DatadogLogSearchResult>, PortError> {
        let response = self
            .http_client
            .request_json(DatadogRequestInput {
                method: Method::POST,
                api_version: DatadogApiVersion::V2,
                path: "/logs/events/search".to_string(),
                query: Vec::new(),
                body: Some(json!({
                    "filter": {
                        "query": params.query,
                        "from": params.from,
                        "to": params.to,
                    },
                    "sort": "-timestamp",
                    "page": {
                        "limit": params.limit,
                    },
                })),
            })
            .await?;

        let data = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut logs = Vec::with_capacity(data.len());
        for log in data {
            logs.push(map_log_search_result(&log));
        }

        Ok(logs)
    }
}

fn map_log_search_result(log: &Value) -> DatadogLogSearchResult {
    let attributes = log.get("attributes");
    DatadogLogSearchResult {
        id: read_string(log.get("id")).unwrap_or_default(),
        timestamp: read_timestamp(attributes).unwrap_or_else(|| "unknown".to_string()),
        service: read_string(attributes.and_then(|value| value.get("service"))),
        status: read_string(attributes.and_then(|value| value.get("status"))),
        message: read_string(attributes.and_then(|value| value.get("message"))),
        attributes: read_object_value(attributes.and_then(|value| value.get("attributes"))),
    }
}

fn read_timestamp(value: Option<&Value>) -> Option<String> {
    read_string(value.and_then(|attributes| attributes.get("timestamp")))
}

fn read_object_value(value: Option<&Value>) -> Option<Value> {
    let object = value.and_then(Value::as_object)?;
    Some(Value::Object(object.clone()))
}

fn read_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_shared::monitoring::datadog::{DatadogLogSearchParams, DatadogLogSearchPort};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DatadogLogSearchAdapter;
    use crate::outbound::datadog::{
        DatadogApiRetryConfig, DatadogHttpClient, DatadogHttpClientConfig,
    };

    #[tokio::test]
    async fn maps_logs_response_into_port_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/logs/events/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {
                        "id": "log-1",
                        "attributes": {
                            "timestamp": "2026-03-04T10:00:00Z",
                            "service": "api",
                            "status": "error",
                            "message": "request failed",
                            "attributes": {
                                "env": "prod"
                            }
                        }
                    },
                    {
                        "id": "log-2",
                        "attributes": {
                            "service": "worker"
                        }
                    }
                ]
            })))
            .mount(&server)
            .await;

        let adapter = DatadogLogSearchAdapter::new(Arc::new(create_client(&server.uri())));
        let results = adapter
            .search_logs(DatadogLogSearchParams {
                query: "service:api".to_string(),
                from: "2026-03-04T09:00:00Z".to_string(),
                to: "2026-03-04T11:00:00Z".to_string(),
                limit: 10,
            })
            .await
            .expect("search logs");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "log-1");
        assert_eq!(results[0].timestamp, "2026-03-04T10:00:00Z");
        assert_eq!(results[0].service.as_deref(), Some("api"));
        assert_eq!(results[0].status.as_deref(), Some("error"));
        assert_eq!(results[0].message.as_deref(), Some("request failed"));
        assert_eq!(results[0].attributes, Some(json!({"env": "prod"})));
        assert_eq!(results[1].timestamp, "unknown");
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
