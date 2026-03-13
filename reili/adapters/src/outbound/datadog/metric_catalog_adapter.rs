use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{DatadogMetricCatalogParams, DatadogMetricCatalogPort};
use reqwest::Method;
use serde_json::Value;

use super::datadog_http_client::{DatadogApiVersion, DatadogHttpClient, DatadogRequestInput};

#[derive(Debug, Clone)]
pub struct DatadogMetricCatalogAdapter {
    http_client: Arc<DatadogHttpClient>,
}

impl DatadogMetricCatalogAdapter {
    pub fn new(http_client: Arc<DatadogHttpClient>) -> Self {
        Self { http_client }
    }
}

#[async_trait]
impl DatadogMetricCatalogPort for DatadogMetricCatalogAdapter {
    async fn list_metrics(
        &self,
        params: DatadogMetricCatalogParams,
    ) -> Result<Vec<String>, PortError> {
        let limit = normalize_limit(params.limit);
        if limit == 0 {
            return Ok(Vec::new());
        }

        let from_epoch_sec = normalize_from_epoch_sec(params.from_epoch_sec);
        let tag_filter = normalize_optional_text(params.tag_filter);

        let mut query = vec![("from".to_string(), from_epoch_sec.to_string())];
        if let Some(filter) = tag_filter {
            query.push(("tag_filter".to_string(), filter));
        }

        let response = self
            .http_client
            .request_json(DatadogRequestInput {
                method: Method::GET,
                api_version: DatadogApiVersion::V1,
                path: "/metrics".to_string(),
                query,
                body: None,
            })
            .await?;

        let metrics = response
            .get("metrics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut items = Vec::new();
        for metric in metrics {
            if let Some(name) = metric
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                items.push(name.to_string());
            }
            if items.len() == limit {
                break;
            }
        }

        Ok(items)
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    let normalized = value?.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    Some(normalized)
}

fn normalize_limit(limit: u32) -> usize {
    usize::try_from(limit).unwrap_or(usize::MAX)
}

fn normalize_from_epoch_sec(from_epoch_sec: i64) -> i64 {
    if from_epoch_sec <= 0 {
        return 0;
    }

    from_epoch_sec
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reili_shared::ports::outbound::{DatadogMetricCatalogParams, DatadogMetricCatalogPort};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DatadogMetricCatalogAdapter;
    use crate::outbound::datadog::{
        DatadogApiRetryConfig, DatadogHttpClient, DatadogHttpClientConfig,
    };

    #[tokio::test]
    async fn returns_metrics_up_to_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/metrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "metrics": [
                    "system.cpu.user",
                    "system.mem.used",
                    "system.net.bytes_sent"
                ]
            })))
            .mount(&server)
            .await;

        let adapter = DatadogMetricCatalogAdapter::new(Arc::new(create_client(&server.uri())));
        let metrics = adapter
            .list_metrics(DatadogMetricCatalogParams {
                from_epoch_sec: 1_710_000_000,
                tag_filter: Some("env:prod".to_string()),
                limit: 2,
            })
            .await
            .expect("list metrics");

        assert_eq!(
            metrics,
            vec!["system.cpu.user".to_string(), "system.mem.used".to_string()]
        );
    }

    #[tokio::test]
    async fn returns_empty_when_limit_is_zero() {
        let adapter = DatadogMetricCatalogAdapter::new(Arc::new(create_client("http://localhost")));
        let metrics = adapter
            .list_metrics(DatadogMetricCatalogParams {
                from_epoch_sec: 1_710_000_000,
                tag_filter: None,
                limit: 0,
            })
            .await
            .expect("list metrics");

        assert!(metrics.is_empty());
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
