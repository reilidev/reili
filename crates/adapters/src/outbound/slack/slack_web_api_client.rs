use reili_core::error::PortError;
use reili_core::secret::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::json_utils::truncate_for_error;

const DEFAULT_BASE_URL: &str = "https://slack.com/api";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackWebApiClientConfig {
    pub bot_token: SecretString,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SlackWebApiClient {
    client: reqwest::Client,
    bot_token: SecretString,
    base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlackApiEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response_metadata: Option<SlackApiResponseMetadata>,
    #[serde(flatten)]
    body: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlackApiResponseMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

impl SlackApiEnvelope {
    fn is_error(&self) -> bool {
        self.ok == Some(false)
    }

    fn error_code(&self) -> String {
        self.error
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("unknown_error")
            .to_string()
    }

    fn metadata_messages(&self) -> &[String] {
        self.response_metadata
            .as_ref()
            .map_or(&[], |metadata| metadata.messages.as_slice())
    }

    fn into_value(self, method_path: &str) -> Result<Value, PortError> {
        serde_json::to_value(self).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to serialize Slack API response JSON: method={method_path} error={error}"
            ))
        })
    }
}

impl SlackWebApiClient {
    pub fn new(config: SlackWebApiClientConfig) -> Result<Self, PortError> {
        let base_url = normalize_base_url(config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL))?;
        if config.bot_token.expose().trim().is_empty() {
            return Err(PortError::invalid_input(
                "Slack bot token must not be empty",
            ));
        }

        let client = reqwest::Client::builder().build().map_err(|error| {
            PortError::new(format!("Failed to build Slack HTTP client: {error}"))
        })?;

        Ok(Self {
            client,
            bot_token: config.bot_token,
            base_url,
        })
    }

    pub async fn post<TPayload>(&self, method: &str, payload: &TPayload) -> Result<Value, PortError>
    where
        TPayload: Serialize + ?Sized,
    {
        let method_path = normalize_method_path(method);
        let url = format!("{}/{method_path}", self.base_url);
        let request = self
            .client
            .post(&url)
            .bearer_auth(self.bot_token.expose())
            .json(payload);

        self.execute(&method_path, request).await
    }

    pub async fn get<TQuery>(&self, method: &str, query: &TQuery) -> Result<Value, PortError>
    where
        TQuery: Serialize + ?Sized,
    {
        let method_path = normalize_method_path(method);
        let url = format!("{}/{method_path}", self.base_url);
        let request = self
            .client
            .get(&url)
            .bearer_auth(self.bot_token.expose())
            .query(query);

        self.execute(&method_path, request).await
    }

    async fn execute(
        &self,
        method_path: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<Value, PortError> {
        let response = request.send().await.map_err(|error| {
            PortError::connection_failed(format!(
                "Slack API request failed: method={method_path} error={error}"
            ))
        })?;

        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to read Slack API response body: method={method_path} error={error}"
            ))
        })?;

        if !status.is_success() {
            return Err(PortError::http_status(
                status.as_u16(),
                format!(
                    "Slack API request failed: method={method_path} status={} body={}",
                    status.as_u16(),
                    truncate_for_error(String::from_utf8_lossy(&bytes).as_ref())
                ),
            ));
        }

        if bytes.is_empty() {
            return Ok(Value::Null);
        }

        let response: SlackApiEnvelope = serde_json::from_slice(&bytes).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse Slack API response JSON: method={method_path} error={error}"
            ))
        })?;

        if response.is_error() {
            let error_code = response.error_code();
            return Err(PortError::service_error(
                error_code.clone(),
                format_slack_service_error_message(
                    method_path,
                    &error_code,
                    response.metadata_messages(),
                ),
            ));
        }

        response.into_value(method_path)
    }
}

fn format_slack_service_error_message(
    method_path: &str,
    error_code: &str,
    metadata_messages: &[String],
) -> String {
    let mut message = format!("Slack API returned error: method={method_path} error={error_code}");
    if metadata_messages.is_empty() {
        return message;
    }

    message.push_str(" messages=");
    message.push_str(&truncate_for_error(&metadata_messages.join(" | ")));
    message
}

fn normalize_base_url(value: &str) -> Result<String, PortError> {
    let normalized = value.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        return Err(PortError::invalid_input("Slack base URL must not be empty"));
    }

    Ok(normalized)
}

fn normalize_method_path(method: &str) -> String {
    method.trim().trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use reili_core::secret::SecretString;
    use reqwest::StatusCode;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{SlackWebApiClient, SlackWebApiClientConfig};

    #[tokio::test]
    async fn posts_json_with_bearer_token() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .and(header("Authorization", "Bearer xoxb-test"))
            .and(body_json(json!({
                "channel": "C123",
                "thread_ts": "1710000000.000001",
                "markdown_text": "hello",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true
            })))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        client
            .post(
                "chat.postMessage",
                &json!({
                    "channel": "C123",
                    "thread_ts": "1710000000.000001",
                    "markdown_text": "hello",
                }),
            )
            .await
            .expect("post slack api");
    }

    #[tokio::test]
    async fn gets_query_params_with_bearer_token() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/conversations.replies"))
            .and(header("Authorization", "Bearer xoxb-test"))
            .and(query_param("channel", "C123"))
            .and(query_param("ts", "1710000000.000001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [],
            })))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        client
            .get(
                "conversations.replies",
                &json!({
                    "channel": "C123",
                    "ts": "1710000000.000001",
                }),
            )
            .await
            .expect("get slack api");
    }

    #[tokio::test]
    async fn returns_error_when_slack_api_response_has_ok_false() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "invalid_auth",
            })))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        let error = client
            .post("chat.postMessage", &json!({}))
            .await
            .expect_err("request should fail");

        assert!(error.message.contains("invalid_auth"));
        assert_eq!(error.service_error_code(), Some("invalid_auth"));
    }

    #[tokio::test]
    async fn includes_response_metadata_messages_in_service_errors() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat.update"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "invalid_arguments",
                "response_metadata": {
                    "messages": [
                        "[ERROR] missing required field: channel",
                        "[ERROR] missing required field: ts"
                    ]
                }
            })))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        let error = client
            .post("chat.update", &json!({}))
            .await
            .expect_err("request should fail");

        assert_eq!(error.service_error_code(), Some("invalid_arguments"));
        assert!(error.message.contains("missing required field: channel"));
        assert!(error.message.contains("missing required field: ts"));
    }

    #[tokio::test]
    async fn returns_error_when_http_status_is_not_successful() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            ))
            .mount(&server)
            .await;

        let client = create_client(&server.uri());
        let error = client
            .post("chat.postMessage", &json!({}))
            .await
            .expect_err("request should fail");

        assert!(error.message.contains("status=500"));
        assert_eq!(error.status_code(), Some(500));
    }

    fn create_client(base_url: &str) -> SlackWebApiClient {
        SlackWebApiClient::new(SlackWebApiClientConfig {
            bot_token: SecretString::from("xoxb-test"),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
