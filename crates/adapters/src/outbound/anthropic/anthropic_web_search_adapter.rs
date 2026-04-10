use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::knowledge::{WebCitation, WebSearchInput, WebSearchPort, WebSearchResult};
use reqwest::Client;
use serde_json::{Value, json};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_WEB_SEARCH_TIMEOUT_MS: u64 = 20_000;
const ANTHROPIC_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";
const DEFAULT_MAX_TOKENS: u64 = 1_024;
const DEFAULT_MAX_USES: u32 = 5;
const MAX_CITATIONS: usize = 10;
const MAX_QUERY_CHARS: usize = 500;
const MAX_SUMMARY_CHARS: usize = 4_000;

pub struct AnthropicWebSearchAdapterConfig {
    pub api_key: String,
    pub model: String,
}

pub struct AnthropicWebSearchAdapter {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicWebSearchAdapter {
    pub fn new(config: AnthropicWebSearchAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(
                DEFAULT_ANTHROPIC_WEB_SEARCH_TIMEOUT_MS,
            ))
            .build()
            .expect("build reqwest client for anthropic web search");

        Self {
            client,
            api_key: config.api_key,
            model: config.model,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    #[cfg(test)]
    fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    fn build_request_body(&self, input: &WebSearchInput) -> Value {
        let prompt = if input.user_location.timezone.is_empty() {
            format!(
                "Summarize the most relevant recent public information for: {}",
                input.query
            )
        } else {
            format!(
                "Summarize the most relevant recent public information for: {}. Use {} as the timezone context for recency.",
                input.query, input.user_location.timezone
            )
        };

        let web_search_tool = json!({
            "type": ANTHROPIC_WEB_SEARCH_TOOL_TYPE,
            "name": "web_search",
            "max_uses": DEFAULT_MAX_USES,
        });

        json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
            "tools": [web_search_tool],
        })
    }
}

#[async_trait]
impl WebSearchPort for AnthropicWebSearchAdapter {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError> {
        if input.query.is_empty() {
            return Err(PortError::new("Web search query must not be empty"));
        }
        if input.query.chars().count() > MAX_QUERY_CHARS {
            return Err(PortError::new(format!(
                "Web search query exceeds {MAX_QUERY_CHARS} characters"
            )));
        }

        let body = self.build_request_body(&input);
        let url = format!("{}/v1/messages", self.base_url);

        let start = std::time::Instant::now();
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await;
        let latency_ms = start.elapsed().as_millis();

        let response = match response {
            Ok(resp) => resp,
            Err(error) => {
                tracing::warn!(
                    latency_ms = latency_ms,
                    error = %error,
                    "Anthropic web search request failed"
                );
                return Ok(soft_temporary_error("Web search timed out"));
            }
        };

        let status = response.status();
        if status.is_client_error() {
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                status = status.as_u16(),
                latency_ms = latency_ms,
                body = body,
                "Anthropic web search client error"
            );
            return Ok(soft_client_error(&extract_error_message(
                &body,
                &format!("Web search returned {status}"),
            )));
        }
        if status.is_server_error() {
            tracing::warn!(
                status = status.as_u16(),
                latency_ms = latency_ms,
                "Anthropic web search server error"
            );
            return Ok(soft_temporary_error(&format!(
                "Web search returned {status}"
            )));
        }

        let response_json: Value = response.json().await.map_err(|error| {
            PortError::new(format!(
                "Failed to parse Anthropic web search response: {error}"
            ))
        })?;

        let result = parse_response(&response_json);

        tracing::info!(
            model = self.model,
            latency_ms = latency_ms,
            citation_count = result.citations.len(),
            "Anthropic web search completed"
        );

        if let Some(usage) = response_json.get("usage") {
            tracing::info!(usage = %usage, "Anthropic web search token usage");
        }

        Ok(result)
    }
}

fn extract_error_message(body: &str, fallback: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn soft_temporary_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: json!({"type": "temporary_error", "message": message}).to_string(),
        citations: Vec::new(),
    }
}

fn soft_client_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: json!({"type": "client_error", "message": message}).to_string(),
        citations: Vec::new(),
    }
}

fn classify_search_tool_error(error_code: &str) -> WebSearchResult {
    match error_code {
        "invalid_input" | "query_too_long" | "max_uses_exceeded" => {
            soft_client_error(&format!("Web search returned {error_code}"))
        }
        "too_many_requests" | "unavailable" => {
            soft_temporary_error(&format!("Web search returned {error_code}"))
        }
        _ => soft_temporary_error(&format!("Web search returned {error_code}")),
    }
}

fn parse_response(response: &Value) -> WebSearchResult {
    let mut summary_text = String::new();
    let mut citations = Vec::new();
    let mut seen_urls = HashSet::new();

    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return WebSearchResult {
            summary_text,
            citations,
        };
    };

    for block in content {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");

        if block_type == "web_search_tool_result" {
            if let Some(error_code) = block
                .get("content")
                .and_then(Value::as_object)
                .and_then(|value| value.get("error_code"))
                .and_then(Value::as_str)
            {
                return classify_search_tool_error(error_code);
            }

            if let Some(results) = block.get("content").and_then(Value::as_array) {
                for result in results {
                    if result.get("type").and_then(Value::as_str) != Some("web_search_result") {
                        continue;
                    }

                    push_citation(
                        &mut citations,
                        &mut seen_urls,
                        result
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        result
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                }
            }
            continue;
        }

        if block_type != "text" {
            continue;
        }

        if let Some(text) = block.get("text").and_then(Value::as_str) {
            summary_text.push_str(text);
        }

        if let Some(text_citations) = block.get("citations").and_then(Value::as_array) {
            for citation in text_citations {
                if citation.get("type").and_then(Value::as_str)
                    != Some("web_search_result_location")
                {
                    continue;
                }

                push_citation(
                    &mut citations,
                    &mut seen_urls,
                    citation
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    citation
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            }
        }
    }

    if summary_text.chars().count() > MAX_SUMMARY_CHARS {
        summary_text = summary_text.chars().take(MAX_SUMMARY_CHARS).collect();
    }

    WebSearchResult {
        summary_text,
        citations,
    }
}

fn push_citation(
    citations: &mut Vec<WebCitation>,
    seen_urls: &mut HashSet<String>,
    url: &str,
    title: &str,
) {
    if url.is_empty() || citations.len() >= MAX_CITATIONS || !seen_urls.insert(url.to_string()) {
        return;
    }

    citations.push(WebCitation {
        title: title.to_string(),
        url: url.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use reili_core::knowledge::{WebSearchInput, WebSearchPort, WebSearchUserLocation};
    use serde_json::{Value, json};

    use super::{
        ANTHROPIC_WEB_SEARCH_TOOL_TYPE, AnthropicWebSearchAdapter, AnthropicWebSearchAdapterConfig,
        DEFAULT_MAX_USES, parse_response,
    };

    #[test]
    fn builds_anthropic_web_search_request_body() {
        let adapter = AnthropicWebSearchAdapter::new(AnthropicWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
        });

        let body = adapter.build_request_body(&WebSearchInput {
            query: "weather in san francisco".to_string(),
            user_location: WebSearchUserLocation {
                timezone: "Asia/Tokyo".to_string(),
            },
        });

        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TOOL_TYPE);
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["tools"][0]["max_uses"], DEFAULT_MAX_USES);
        assert!(body["tools"][0].get("user_location").is_none());
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .expect("message content")
                .contains("Asia/Tokyo")
        );
    }

    #[test]
    fn parses_web_search_response() {
        let parsed = parse_response(&json!({
            "content": [
                {
                    "type": "text",
                    "text": "I'll search for that."
                },
                {
                    "type": "server_tool_use",
                    "id": "srvtoolu_1",
                    "name": "web_search",
                    "input": {
                        "query": "latest api outage"
                    }
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_1",
                    "content": [
                        {
                            "type": "web_search_result",
                            "url": "https://status.example.com",
                            "title": "Status Page"
                        },
                        {
                            "type": "web_search_result",
                            "url": "https://blog.example.com/post",
                            "title": "Incident report"
                        }
                    ]
                },
                {
                    "type": "text",
                    "text": "There is an ongoing incident.",
                    "citations": [
                        {
                            "type": "web_search_result_location",
                            "url": "https://status.example.com",
                            "title": "Status Page"
                        }
                    ]
                }
            ]
        }));

        assert_eq!(
            parsed.summary_text,
            "I'll search for that.There is an ongoing incident."
        );
        assert_eq!(parsed.citations.len(), 2);
    }

    #[test]
    fn converts_search_tool_errors_to_soft_errors() {
        let parsed = parse_response(&json!({
            "content": [
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_1",
                    "content": {
                        "type": "web_search_tool_result_error",
                        "error_code": "max_uses_exceeded"
                    }
                }
            ]
        }));

        let body: Value = serde_json::from_str(&parsed.summary_text).expect("valid JSON");
        assert_eq!(body["type"], "client_error");
        assert_eq!(body["message"], "Web search returned max_uses_exceeded");
    }

    #[tokio::test]
    async fn search_uses_configured_base_url() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "content": [{
                    "type": "text",
                    "text": "ok"
                }]
            })))
            .mount(&server)
            .await;

        let adapter = AnthropicWebSearchAdapter::new(AnthropicWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
        })
        .with_base_url(server.uri());

        let result = adapter
            .search(WebSearchInput {
                query: "anthropic news".to_string(),
                user_location: WebSearchUserLocation {
                    timezone: String::new(),
                },
            })
            .await
            .expect("search should succeed");

        assert_eq!(result.summary_text, "ok");
    }
}
