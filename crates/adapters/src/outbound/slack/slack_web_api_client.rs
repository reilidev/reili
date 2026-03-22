use reili_core::error::PortError;
use serde::Serialize;
use serde_json::Value;

use crate::json_utils::{read_non_empty_json_string, truncate_for_error};

const DEFAULT_BASE_URL: &str = "https://slack.com/api";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackWebApiClientConfig {
    pub bot_token: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SlackWebApiClient {
    client: reqwest::Client,
    bot_token: String,
    base_url: String,
}

impl SlackWebApiClient {
    pub fn new(config: SlackWebApiClientConfig) -> Result<Self, PortError> {
        let base_url = normalize_base_url(config.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL))?;
        if config.bot_token.trim().is_empty() {
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

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.bot_token)
            .json(payload)
            .send()
            .await
            .map_err(|error| {
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

        let json: Value = serde_json::from_slice(&bytes).map_err(|error| {
            PortError::invalid_response(format!(
                "Failed to parse Slack API response JSON: method={method_path} error={error}"
            ))
        })?;

        if json.get("ok").and_then(Value::as_bool) == Some(false) {
            let error_code = read_non_empty_json_string(json.get("error"))
                .unwrap_or_else(|| "unknown_error".to_string());
            return Err(PortError::service_error(
                error_code.clone(),
                format!("Slack API returned error: method={method_path} error={error_code}"),
            ));
        }

        Ok(json)
    }
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
    use reqwest::StatusCode;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
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
            bot_token: "xoxb-test".to_string(),
            base_url: Some(base_url.to_string()),
        })
        .expect("create slack api client")
    }
}
