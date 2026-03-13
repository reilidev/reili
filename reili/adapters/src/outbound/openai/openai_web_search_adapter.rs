use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{
    WebCitation, WebSearchExecution, WebSearchInput, WebSearchPort, WebSearchResult,
};

const MAX_CITATIONS: usize = 10;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_QUERY_CHARS: usize = 500;

pub struct OpenAiWebSearchAdapterConfig {
    pub api_key: String,
    pub model: String,
    pub timeout_ms: u64,
}

pub struct OpenAiWebSearchAdapter {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAiWebSearchAdapter {
    pub fn new(config: OpenAiWebSearchAdapterConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("build reqwest client for web search");

        Self {
            client,
            api_key: config.api_key,
            model: config.model,
            base_url: "https://api.openai.com".to_string(),
        }
    }

    #[cfg(test)]
    fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    fn build_request_body(&self, input: &WebSearchInput) -> Value {
        let mut web_search_tool: Value = json!({
            "type": "web_search",
            "search_context_size": "medium"
        });

        if !input.user_location.timezone.is_empty() {
            web_search_tool["user_location"] = json!({
                "type": "approximate",
                "country": "",
                "city": "",
                "region": "",
                "timezone": input.user_location.timezone
            });
        }

        json!({
            "model": self.model,
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": format!(
                                "Summarize the most relevant recent public information for: {}",
                                input.query
                            )
                        }
                    ]
                }
            ],
            "tools": [web_search_tool],
            "include": ["web_search_call.action.sources"],
            "text": {
                "format": {
                    "type": "text"
                }
            }
        })
    }
}

#[async_trait]
impl WebSearchPort for OpenAiWebSearchAdapter {
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
        let url = format!("{}/v1/responses", self.base_url);

        let start = std::time::Instant::now();
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await;
        let latency_ms = start.elapsed().as_millis();

        let response = match response {
            Ok(resp) => resp,
            Err(err) => {
                tracing::warn!(
                    latency_ms = latency_ms,
                    error = %err,
                    "Web search request failed"
                );
                return Ok(soft_temporary_error("Web search timed out"));
            }
        };

        let status = response.status();
        if status.is_client_error() {
            let error_body = response.text().await.unwrap_or_default();
            tracing::warn!(
                status = status.as_u16(),
                latency_ms = latency_ms,
                body = error_body,
                "Web search client error"
            );
            return Ok(soft_client_error(&format!("Web search returned {status}")));
        }
        if status.is_server_error() {
            tracing::warn!(
                status = status.as_u16(),
                latency_ms = latency_ms,
                "Web search server error"
            );
            return Ok(soft_temporary_error(&format!(
                "Web search returned {status}"
            )));
        }

        let response_json: Value = response
            .json()
            .await
            .map_err(|err| PortError::new(format!("Failed to parse web search response: {err}")))?;

        let result = parse_response(&response_json);

        tracing::info!(
            model = self.model,
            latency_ms = latency_ms,
            citation_count = result.citations.len(),
            search_count = result.searches.len(),
            "Web search completed"
        );

        if let Some(usage) = response_json.get("usage") {
            tracing::info!(usage = %usage, "Web search token usage");
        }

        Ok(result)
    }
}

fn soft_temporary_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: format!("{{\"type\":\"temporary_error\",\"message\":\"{message}\"}}"),
        citations: Vec::new(),
        searches: Vec::new(),
    }
}

fn soft_client_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: format!("{{\"type\":\"client_error\",\"message\":\"{message}\"}}"),
        citations: Vec::new(),
        searches: Vec::new(),
    }
}

fn parse_response(response: &Value) -> WebSearchResult {
    let mut summary_text = String::new();
    let mut citations = Vec::new();
    let mut searches = Vec::new();
    let mut seen_urls = HashSet::new();

    let output = response.get("output").and_then(Value::as_array);
    let items = match output {
        Some(items) => items,
        None => {
            return WebSearchResult {
                summary_text: String::new(),
                citations,
                searches,
            };
        }
    };

    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

        if item_type == "web_search_call"
            && let Some(action) = item.get("action")
        {
            let query = action
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let source_count = action
                .get("sources")
                .and_then(Value::as_array)
                .map(|s| s.len() as u32)
                .unwrap_or(0);
            searches.push(WebSearchExecution {
                query,
                source_count,
            });
        }

        if item_type == "message"
            && let Some(content) = item.get("content").and_then(Value::as_array)
        {
            for part in content {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");

                if part_type == "output_text" {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        summary_text.push_str(text);
                    }

                    if let Some(annotations) = part.get("annotations").and_then(Value::as_array) {
                        for annotation in annotations {
                            let ann_type =
                                annotation.get("type").and_then(Value::as_str).unwrap_or("");
                            if ann_type == "url_citation" {
                                let url = annotation
                                    .get("url")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let title = annotation
                                    .get("title")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                if !url.is_empty() && seen_urls.insert(url.clone()) {
                                    citations.push(WebCitation { title, url });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Response guards
    citations.truncate(MAX_CITATIONS);
    if summary_text.chars().count() > MAX_SUMMARY_CHARS {
        let truncated: String = summary_text.chars().take(MAX_SUMMARY_CHARS).collect();
        summary_text = truncated;
    }

    WebSearchResult {
        summary_text,
        citations,
        searches,
    }
}

#[cfg(test)]
mod tests {
    use reili_shared::ports::outbound::WebSearchUserLocation;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn default_location() -> WebSearchUserLocation {
        WebSearchUserLocation {
            timezone: "Asia/Tokyo".to_string(),
        }
    }

    fn sample_response_json() -> Value {
        json!({
            "output": [
                {
                    "type": "web_search_call",
                    "action": {
                        "query": "latest openai news",
                        "sources": [
                            {"url": "https://example.com/1", "title": "Source 1"},
                            {"url": "https://example.com/2", "title": "Source 2"}
                        ]
                    }
                },
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Here is a summary of recent news.",
                            "annotations": [
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/1",
                                    "title": "Example Article 1"
                                },
                                {
                                    "type": "url_citation",
                                    "url": "https://example.com/2",
                                    "title": "Example Article 2"
                                }
                            ]
                        }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        })
    }

    #[test]
    fn builds_request_body_with_web_search_tool() {
        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 20_000,
        });

        let input = WebSearchInput {
            query: "test query".to_string(),
            user_location: default_location(),
        };

        let body = adapter.build_request_body(&input);
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "web_search");
        assert_eq!(tools[0]["search_context_size"], "medium");
        assert_eq!(tools[0]["user_location"]["timezone"], "Asia/Tokyo");
        assert_eq!(tools[0]["user_location"]["country"], "");

        let include = body["include"].as_array().expect("include array");
        assert_eq!(include[0], "web_search_call.action.sources");
    }

    #[test]
    fn parses_response_extracts_text_and_citations() {
        let response = sample_response_json();
        let result = parse_response(&response);

        assert_eq!(result.summary_text, "Here is a summary of recent news.");
        assert_eq!(result.citations.len(), 2);
        assert_eq!(result.citations[0].url, "https://example.com/1");
        assert_eq!(result.citations[1].title, "Example Article 2");
        assert_eq!(result.searches.len(), 1);
        assert_eq!(result.searches[0].query, "latest openai news");
        assert_eq!(result.searches[0].source_count, 2);
    }

    #[test]
    fn deduplicates_citation_urls() {
        let response = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Summary",
                            "annotations": [
                                {"type": "url_citation", "url": "https://dup.com", "title": "First"},
                                {"type": "url_citation", "url": "https://dup.com", "title": "Second"}
                            ]
                        }
                    ]
                }
            ]
        });
        let result = parse_response(&response);
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].title, "First");
    }

    #[test]
    fn truncates_summary_text_at_4000_chars() {
        let long_text = "a".repeat(5_000);
        let response = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": long_text
                        }
                    ]
                }
            ]
        });
        let result = parse_response(&response);
        assert_eq!(result.summary_text.len(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn truncates_citations_at_max_ten() {
        let annotations: Vec<Value> = (0..15)
            .map(|i| {
                json!({
                    "type": "url_citation",
                    "url": format!("https://example.com/{i}"),
                    "title": format!("Title {i}")
                })
            })
            .collect();
        let response = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Summary",
                            "annotations": annotations
                        }
                    ]
                }
            ]
        });
        let result = parse_response(&response);
        assert_eq!(result.citations.len(), MAX_CITATIONS);
    }

    #[tokio::test]
    async fn sends_request_and_parses_successful_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_response_json()))
            .mount(&server)
            .await;

        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 5_000,
        })
        .with_base_url(server.uri());

        let result = adapter
            .search(WebSearchInput {
                query: "test query".to_string(),
                user_location: default_location(),
            })
            .await
            .expect("search should succeed");

        assert_eq!(result.summary_text, "Here is a summary of recent news.");
        assert_eq!(result.citations.len(), 2);
    }

    #[tokio::test]
    async fn returns_soft_error_on_server_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 5_000,
        })
        .with_base_url(server.uri());

        let result = adapter
            .search(WebSearchInput {
                query: "test".to_string(),
                user_location: default_location(),
            })
            .await
            .expect("should return soft error, not Err");

        assert!(result.summary_text.contains("temporary_error"));
    }

    #[tokio::test]
    async fn returns_soft_error_on_client_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;

        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 5_000,
        })
        .with_base_url(server.uri());

        let result = adapter
            .search(WebSearchInput {
                query: "test".to_string(),
                user_location: default_location(),
            })
            .await
            .expect("should return soft error, not Err");

        assert!(result.summary_text.contains("client_error"));
    }

    #[tokio::test]
    async fn rejects_empty_query() {
        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 5_000,
        });

        let err = adapter
            .search(WebSearchInput {
                query: String::new(),
                user_location: default_location(),
            })
            .await
            .expect_err("should reject empty query");

        assert!(err.message.contains("empty"));
    }

    #[tokio::test]
    async fn rejects_query_exceeding_max_length() {
        let adapter = OpenAiWebSearchAdapter::new(OpenAiWebSearchAdapterConfig {
            api_key: "test-key".to_string(),
            model: "gpt-5".to_string(),
            timeout_ms: 5_000,
        });

        let long_query = "a".repeat(501);
        let err = adapter
            .search(WebSearchInput {
                query: long_query,
                user_location: default_location(),
            })
            .await
            .expect_err("should reject long query");

        assert!(err.message.contains("500"));
    }
}
