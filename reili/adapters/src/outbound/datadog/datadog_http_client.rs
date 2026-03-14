use std::time::Duration;

use reili_core::error::PortError;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::json_utils::truncate_for_error;

use super::DatadogApiRetryConfig;

const DEFAULT_MAX_RESPONSE_BYTES: usize = 100 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatadogApiVersion {
    V1,
    V2,
}

#[derive(Debug, Clone)]
pub struct DatadogRequestInput {
    pub method: Method,
    pub api_version: DatadogApiVersion,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct DatadogHttpClientConfig {
    pub api_key: String,
    pub app_key: String,
    pub site: String,
    pub retry: DatadogApiRetryConfig,
    pub max_response_bytes: usize,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DatadogHttpClient {
    client: reqwest::Client,
    api_key: String,
    app_key: String,
    base_url: String,
    retry: DatadogApiRetryConfig,
    max_response_bytes: usize,
}

impl DatadogHttpClient {
    pub fn new(config: DatadogHttpClientConfig) -> Result<Self, PortError> {
        let client = reqwest::Client::builder().build().map_err(|error| {
            PortError::new(format!("Failed to build Datadog HTTP client: {error}"))
        })?;
        let base_url = resolve_base_url(&config)?;
        let max_response_bytes = if config.max_response_bytes == 0 {
            DEFAULT_MAX_RESPONSE_BYTES
        } else {
            config.max_response_bytes
        };

        Ok(Self {
            client,
            api_key: config.api_key,
            app_key: config.app_key,
            base_url,
            retry: config.retry,
            max_response_bytes,
        })
    }

    pub async fn request_json(&self, input: DatadogRequestInput) -> Result<Value, PortError> {
        let mut failed_attempts = 0_u32;

        loop {
            match self.send_once(&input).await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    if self.should_retry(failed_attempts, &error) {
                        let backoff = self.compute_backoff_duration(failed_attempts);
                        tokio::time::sleep(backoff).await;
                        failed_attempts = failed_attempts.saturating_add(1);
                        continue;
                    }

                    return Err(PortError::new(error.message));
                }
            }
        }
    }

    async fn send_once(&self, input: &DatadogRequestInput) -> Result<Value, RequestFailure> {
        let url = self.build_url(input);
        let mut request_builder = self
            .client
            .request(input.method.clone(), &url)
            .header("DD-API-KEY", &self.api_key)
            .header("DD-APPLICATION-KEY", &self.app_key);

        if !input.query.is_empty() {
            request_builder = request_builder.query(&input.query);
        }

        if let Some(body) = input.body.as_ref() {
            request_builder = request_builder.json(body);
        }

        let response = request_builder
            .send()
            .await
            .map_err(|error| RequestFailure {
                message: format!("Datadog API request failed: {error}"),
                retriable: true,
            })?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| RequestFailure {
            message: format!("Failed to read Datadog API response body: {error}"),
            retriable: true,
        })?;

        if bytes.len() > self.max_response_bytes {
            return Err(RequestFailure {
                message: format!(
                    "Datadog API response exceeded size limit: {} bytes (limit: {} bytes)",
                    bytes.len(),
                    self.max_response_bytes
                ),
                retriable: false,
            });
        }

        if !status.is_success() {
            return Err(RequestFailure {
                message: format!(
                    "Datadog API request failed: status={} body={}",
                    status.as_u16(),
                    truncate_for_error(String::from_utf8_lossy(&bytes).as_ref())
                ),
                retriable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
            });
        }

        if bytes.is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_slice(&bytes).map_err(|error| RequestFailure {
            message: format!("Failed to parse Datadog API response JSON: {error}"),
            retriable: false,
        })
    }

    fn should_retry(&self, failed_attempts: u32, error: &RequestFailure) -> bool {
        self.retry.enabled && error.retriable && failed_attempts < self.retry.max_retries
    }

    fn compute_backoff_duration(&self, failed_attempts: u32) -> Duration {
        let base_seconds = u64::from(self.retry.backoff_base_seconds);
        let multiplier = u64::from(self.retry.backoff_multiplier).max(1);
        let factor = multiplier.saturating_pow(failed_attempts);

        Duration::from_secs(base_seconds.saturating_mul(factor))
    }

    fn build_url(&self, input: &DatadogRequestInput) -> String {
        let api_prefix = match input.api_version {
            DatadogApiVersion::V1 => "/api/v1",
            DatadogApiVersion::V2 => "/api/v2",
        };
        let path = normalize_path(&input.path);

        format!("{}{api_prefix}{path}", self.base_url)
    }
}

#[derive(Debug)]
struct RequestFailure {
    message: String,
    retriable: bool,
}

fn resolve_base_url(config: &DatadogHttpClientConfig) -> Result<String, PortError> {
    if let Some(base_url) = config.base_url.as_ref() {
        return normalize_base_url(base_url);
    }

    let normalized_site = config.site.trim();
    if normalized_site.is_empty() {
        return Err(PortError::new("Datadog site must not be empty"));
    }

    normalize_base_url(format!("https://api.{normalized_site}").as_str())
}

fn normalize_base_url(value: &str) -> Result<String, PortError> {
    let normalized = value.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        return Err(PortError::new("Datadog base URL must not be empty"));
    }

    Ok(normalized)
}

fn normalize_path(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }

    format!("/{path}")
}

#[cfg(test)]
mod tests {
    use reqwest::Method;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        DatadogApiRetryConfig, DatadogApiVersion, DatadogHttpClient, DatadogHttpClientConfig,
        DatadogRequestInput,
    };

    #[tokio::test]
    async fn sends_auth_headers_and_parses_response_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/events"))
            .and(header("DD-API-KEY", "dd-api-key"))
            .and(header("DD-APPLICATION-KEY", "dd-app-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;

        let client = create_client(&server.uri(), retry_disabled());
        let response = client
            .request_json(DatadogRequestInput {
                method: Method::GET,
                api_version: DatadogApiVersion::V2,
                path: "/events".to_string(),
                query: Vec::new(),
                body: None,
            })
            .await
            .expect("request succeeds");

        assert_eq!(response["ok"], true);
    }

    #[tokio::test]
    async fn retries_retriable_failures_until_retry_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/events"))
            .respond_with(ResponseTemplate::new(500).set_body_string("temporary failure"))
            .expect(3)
            .mount(&server)
            .await;

        let client = create_client(
            &server.uri(),
            DatadogApiRetryConfig {
                enabled: true,
                max_retries: 2,
                backoff_base_seconds: 0,
                backoff_multiplier: 2,
            },
        );

        let result = client
            .request_json(DatadogRequestInput {
                method: Method::GET,
                api_version: DatadogApiVersion::V2,
                path: "/events".to_string(),
                query: Vec::new(),
                body: None,
            })
            .await;

        let error = result.expect_err("all attempts fail");
        assert!(error.message.contains("status=500"));
    }

    #[tokio::test]
    async fn rejects_response_that_exceeds_size_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/events"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(256)))
            .mount(&server)
            .await;

        let client = DatadogHttpClient::new(DatadogHttpClientConfig {
            api_key: "dd-api-key".to_string(),
            app_key: "dd-app-key".to_string(),
            site: "datadoghq.com".to_string(),
            retry: retry_disabled(),
            max_response_bytes: 100,
            base_url: Some(server.uri()),
        })
        .expect("build datadog client");

        let result = client
            .request_json(DatadogRequestInput {
                method: Method::GET,
                api_version: DatadogApiVersion::V2,
                path: "/events".to_string(),
                query: Vec::new(),
                body: None,
            })
            .await;

        let error = result.expect_err("response is too large");
        assert!(error.message.contains("exceeded size limit"));
    }

    fn create_client(base_url: &str, retry: DatadogApiRetryConfig) -> DatadogHttpClient {
        DatadogHttpClient::new(DatadogHttpClientConfig {
            api_key: "dd-api-key".to_string(),
            app_key: "dd-app-key".to_string(),
            site: "datadoghq.com".to_string(),
            retry,
            max_response_bytes: 0,
            base_url: Some(base_url.to_string()),
        })
        .expect("build datadog client")
    }

    fn retry_disabled() -> DatadogApiRetryConfig {
        DatadogApiRetryConfig {
            enabled: false,
            max_retries: 0,
            backoff_base_seconds: 2,
            backoff_multiplier: 2,
        }
    }
}
