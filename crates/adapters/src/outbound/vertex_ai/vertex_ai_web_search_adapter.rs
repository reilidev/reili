use std::collections::HashSet;

use async_trait::async_trait;
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_api::model::HttpBody;
use reili_core::error::PortError;
use reili_core::knowledge::{
    WebCitation, WebSearchExecution, WebSearchInput, WebSearchPort, WebSearchResult,
};
use serde_json::{Value, json};

use super::{ANTHROPIC_PUBLISHER, ANTHROPIC_VERTEX_VERSION, vertex_ai_base_url};

const VERTEX_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";
const DEFAULT_MAX_TOKENS: u64 = 1_024;
const DEFAULT_MAX_USES: u32 = 5;
const MAX_CITATIONS: usize = 10;
const MAX_QUERY_CHARS: usize = 500;
const MAX_SUMMARY_CHARS: usize = 4_000;

pub struct VertexAiWebSearchAdapterConfig {
    pub project_id: String,
    pub location: String,
    pub model_id: String,
}

pub struct VertexAiWebSearchAdapter {
    prediction_service: PredictionService,
    project_id: String,
    location: String,
    model_id: String,
}

impl VertexAiWebSearchAdapter {
    pub async fn new(config: VertexAiWebSearchAdapterConfig) -> Result<Self, String> {
        let prediction_service = PredictionService::builder()
            .with_endpoint(vertex_ai_base_url(&config.location))
            .build()
            .await
            .map_err(|error| error.to_string())?;

        Ok(Self {
            prediction_service,
            project_id: config.project_id,
            location: config.location,
            model_id: config.model_id,
        })
    }

    fn build_request_body(input: &WebSearchInput) -> Value {
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

        json!({
            "anthropic_version": ANTHROPIC_VERTEX_VERSION,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
            "tools": [{
                "type": VERTEX_WEB_SEARCH_TOOL_TYPE,
                "name": "web_search",
                "max_uses": DEFAULT_MAX_USES,
            }],
        })
    }

    async fn raw_predict(&self, body: &Value) -> Result<Vec<u8>, String> {
        let body = serde_json::to_vec(body).map_err(|error| error.to_string())?;
        let http_body = HttpBody::new()
            .set_content_type("application/json")
            .set_data(body);

        self.prediction_service
            .raw_predict()
            .set_endpoint(vertex_model_endpoint(
                &self.project_id,
                &self.location,
                &self.model_id,
            ))
            .set_http_body(http_body)
            .send()
            .await
            .map(|response| response.data.to_vec())
            .map_err(|error| error.to_string())
    }
}

#[async_trait]
impl WebSearchPort for VertexAiWebSearchAdapter {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError> {
        if input.query.is_empty() {
            return Err(PortError::new("Web search query must not be empty"));
        }
        if input.query.chars().count() > MAX_QUERY_CHARS {
            return Err(PortError::new(format!(
                "Web search query exceeds {MAX_QUERY_CHARS} characters"
            )));
        }

        let response_bytes = match self.raw_predict(&Self::build_request_body(&input)).await {
            Ok(response_bytes) => response_bytes,
            Err(error) => return Ok(classify_provider_error(&error)),
        };

        let response_json: Value = serde_json::from_slice(&response_bytes).map_err(|error| {
            PortError::new(format!(
                "Failed to parse Vertex AI web search response: {error}"
            ))
        })?;

        Ok(parse_response(&response_json))
    }
}

fn classify_provider_error(message: &str) -> WebSearchResult {
    let normalized = message.to_ascii_uppercase();
    if normalized.contains("FAILED_PRECONDITION")
        || normalized.contains("PERMISSION_DENIED")
        || normalized.contains("INVALID_ARGUMENT")
        || normalized.contains("VPCSC")
    {
        soft_client_error(message)
    } else {
        soft_temporary_error(message)
    }
}

fn soft_temporary_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: json!({"type": "temporary_error", "message": message}).to_string(),
        citations: Vec::new(),
        searches: Vec::new(),
    }
}

fn soft_client_error(message: &str) -> WebSearchResult {
    WebSearchResult {
        summary_text: json!({"type": "client_error", "message": message}).to_string(),
        citations: Vec::new(),
        searches: Vec::new(),
    }
}

fn parse_response(response: &Value) -> WebSearchResult {
    let mut summary_text = String::new();
    let mut citations = Vec::new();
    let mut searches = Vec::new();
    let mut seen_urls = HashSet::new();

    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return WebSearchResult {
            summary_text,
            citations,
            searches,
        };
    };

    for block in content {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");

        if block_type == "server_tool_use" {
            let query = block
                .get("input")
                .and_then(|value| value.get("query"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            searches.push(WebSearchExecution {
                query,
                source_count: 0,
            });
        }

        if block_type == "web_search_tool_result"
            && let Some(results) = block.get("content").and_then(Value::as_array)
        {
            let source_count = results
                .iter()
                .filter(|result| {
                    result.get("type").and_then(Value::as_str) == Some("web_search_result")
                })
                .count() as u32;

            if let Some(search) = searches.last_mut() {
                search.source_count = source_count;
            }

            for result in results {
                if result.get("type").and_then(Value::as_str) != Some("web_search_result") {
                    continue;
                }

                let url = result
                    .get("uri")
                    .or_else(|| result.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let title = result
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                if !url.is_empty()
                    && seen_urls.insert(url.clone())
                    && citations.len() < MAX_CITATIONS
                {
                    citations.push(WebCitation { title, url });
                }
            }
        }

        if block_type == "text"
            && let Some(text) = block.get("text").and_then(Value::as_str)
        {
            summary_text.push_str(text);
        }
    }

    if summary_text.chars().count() > MAX_SUMMARY_CHARS {
        summary_text = summary_text.chars().take(MAX_SUMMARY_CHARS).collect();
    }

    WebSearchResult {
        summary_text,
        citations,
        searches,
    }
}

fn vertex_model_endpoint(project_id: &str, location: &str, model: &str) -> String {
    format!(
        "projects/{project_id}/locations/{location}/publishers/{ANTHROPIC_PUBLISHER}/models/{model}:rawPredict"
    )
}

#[cfg(test)]
mod tests {
    use reili_core::knowledge::{WebSearchInput, WebSearchUserLocation};
    use serde_json::{Value, json};

    use super::{
        ANTHROPIC_VERTEX_VERSION, DEFAULT_MAX_USES, VERTEX_WEB_SEARCH_TOOL_TYPE,
        VertexAiWebSearchAdapter, parse_response, soft_client_error,
    };

    #[test]
    fn builds_vertex_web_search_request_body() {
        let body = VertexAiWebSearchAdapter::build_request_body(&WebSearchInput {
            query: "weather in san francisco".to_string(),
            user_location: WebSearchUserLocation {
                timezone: "Asia/Tokyo".to_string(),
            },
        });

        assert_eq!(
            body.get("anthropic_version"),
            Some(&json!(ANTHROPIC_VERTEX_VERSION))
        );
        assert_eq!(body["tools"][0]["type"], VERTEX_WEB_SEARCH_TOOL_TYPE);
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["tools"][0]["max_uses"], DEFAULT_MAX_USES);
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
                    "text": "I need to search."
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
                            "uri": "https://status.example.com",
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
                    "text": "There is an ongoing incident."
                }
            ]
        }));

        assert_eq!(
            parsed.summary_text,
            "I need to search.There is an ongoing incident."
        );
        assert_eq!(parsed.citations.len(), 2);
        assert_eq!(parsed.searches.len(), 1);
        assert_eq!(parsed.searches[0].query, "latest api outage");
        assert_eq!(parsed.searches[0].source_count, 2);
    }

    #[test]
    fn classifies_precondition_errors_as_client_errors() {
        let result = soft_client_error("FAILED_PRECONDITION");
        let parsed: Value = serde_json::from_str(&result.summary_text).expect("valid JSON");
        assert_eq!(parsed["type"], "client_error");
        assert_eq!(parsed["message"], "FAILED_PRECONDITION");
    }
}
