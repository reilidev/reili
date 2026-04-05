use std::collections::HashSet;
use std::time::Instant;

use async_trait::async_trait;
use google_cloud_aiplatform_v1 as vertexai;
use reili_core::error::PortError;
use reili_core::knowledge::{
    WebCitation, WebSearchExecution, WebSearchInput, WebSearchPort, WebSearchResult,
};
use rig_vertexai::Client as VertexAiGeminiClient;
use serde_json::json;

const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 1_024;
const MAX_CITATIONS: usize = 10;
const MAX_QUERY_CHARS: usize = 500;
const MAX_SUMMARY_CHARS: usize = 4_000;

pub struct VertexAiWebSearchAdapterConfig {
    pub client: VertexAiGeminiClient,
    pub model_id: String,
}

pub struct VertexAiWebSearchAdapter {
    client: VertexAiGeminiClient,
    model_id: String,
}

impl VertexAiWebSearchAdapter {
    pub fn new(config: VertexAiWebSearchAdapterConfig) -> Self {
        Self {
            client: config.client,
            model_id: config.model_id,
        }
    }

    fn model_path(&self) -> String {
        format!(
            "projects/{}/locations/{}/publishers/google/models/{}",
            self.client.project(),
            self.client.location(),
            self.model_id
        )
    }
}

#[async_trait]
impl WebSearchPort for VertexAiWebSearchAdapter {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError> {
        validate_query(&input.query)?;

        let prompt = build_search_prompt(&input);
        let request_content = vertexai::model::Content::new()
            .set_role("user")
            .set_parts([vertexai::model::Part::new().set_text(prompt)]);
        let generation_config = vertexai::model::GenerationConfig::new()
            .set_candidate_count(1)
            .set_max_output_tokens(DEFAULT_MAX_OUTPUT_TOKENS);

        let start = Instant::now();
        let response = self
            .client
            .get_inner()
            .await
            .generate_content()
            .set_model(self.model_path())
            .set_contents([request_content])
            .set_generation_config(generation_config)
            .set_tools([vertexai::model::Tool::new()
                .set_google_search(vertexai::model::tool::GoogleSearch::new())])
            .send()
            .await;
        let latency_ms = start.elapsed().as_millis();

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(
                    model = self.model_id,
                    latency_ms = latency_ms,
                    error = %error,
                    "Vertex AI web search request failed"
                );
                return Ok(classify_api_error(&error.to_string()));
            }
        };

        let result = parse_response(&response);

        tracing::info!(
            model = self.model_id,
            latency_ms = latency_ms,
            citation_count = result.citations.len(),
            search_count = result.searches.len(),
            "Vertex AI web search completed"
        );

        if let Some(usage) = response.usage_metadata.as_ref() {
            tracing::info!(usage = ?usage, "Vertex AI web search token usage");
        }

        Ok(result)
    }
}

fn validate_query(query: &str) -> Result<(), PortError> {
    if query.is_empty() {
        return Err(PortError::new("Web search query must not be empty"));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(PortError::new(format!(
            "Web search query exceeds {MAX_QUERY_CHARS} characters"
        )));
    }

    Ok(())
}

fn build_search_prompt(input: &WebSearchInput) -> String {
    if input.user_location.timezone.is_empty() {
        return format!(
            "Summarize the most relevant recent public information for: {}",
            input.query
        );
    }

    format!(
        "Summarize the most relevant recent public information for: {}. Use {} as the timezone context for recency.",
        input.query, input.user_location.timezone
    )
}

fn classify_api_error(message: &str) -> WebSearchResult {
    let normalized = message.to_ascii_uppercase();

    if normalized.contains("INVALID_ARGUMENT")
        || normalized.contains("FAILED_PRECONDITION")
        || normalized.contains("PERMISSION_DENIED")
        || normalized.contains("UNAUTHENTICATED")
    {
        return soft_client_error(message);
    }

    soft_temporary_error(message)
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

fn parse_response(response: &vertexai::model::GenerateContentResponse) -> WebSearchResult {
    let Some(candidate) = response
        .candidates
        .iter()
        .find(|candidate| {
            candidate
                .content
                .as_ref()
                .is_some_and(|content| content.parts.iter().any(|part| part.text().is_some()))
                || candidate.grounding_metadata.is_some()
        })
        .or_else(|| response.candidates.first())
    else {
        return response
            .prompt_feedback
            .as_ref()
            .map_or_else(empty_result, prompt_feedback_result);
    };

    let mut summary_text = String::new();
    if let Some(content) = candidate.content.as_ref() {
        for part in &content.parts {
            if let Some(text) = part.text() {
                summary_text.push_str(text);
            }
        }
    }
    let summary_text = truncate_chars(summary_text, MAX_SUMMARY_CHARS);
    let (citations, searches) = candidate
        .grounding_metadata
        .as_ref()
        .map(parse_grounding_metadata)
        .unwrap_or_default();

    WebSearchResult {
        summary_text,
        citations,
        searches,
    }
}

fn prompt_feedback_result(
    prompt_feedback: &vertexai::model::generate_content_response::PromptFeedback,
) -> WebSearchResult {
    if !prompt_feedback.block_reason_message.is_empty() {
        return soft_client_error(&prompt_feedback.block_reason_message);
    }

    if let Some(block_reason) = prompt_feedback.block_reason.name() {
        return soft_client_error(&format!("Web search blocked: {block_reason}"));
    }

    soft_client_error("Web search returned no candidates")
}

fn empty_result() -> WebSearchResult {
    WebSearchResult {
        summary_text: String::new(),
        citations: Vec::new(),
        searches: Vec::new(),
    }
}

fn parse_grounding_metadata(
    grounding_metadata: &vertexai::model::GroundingMetadata,
) -> (Vec<WebCitation>, Vec<WebSearchExecution>) {
    let citations = collect_citations(grounding_metadata);
    let source_count = citations.len().try_into().unwrap_or(u32::MAX);
    let searches = grounding_metadata
        .web_search_queries
        .iter()
        .filter_map(|query| {
            let query = query.trim();
            (!query.is_empty()).then(|| WebSearchExecution {
                query: query.to_string(),
                source_count,
            })
        })
        .collect();

    (citations, searches)
}

fn collect_citations(grounding_metadata: &vertexai::model::GroundingMetadata) -> Vec<WebCitation> {
    let mut citations = Vec::new();
    let mut seen_urls = HashSet::new();

    for chunk in &grounding_metadata.grounding_chunks {
        let Some(citation) = chunk_to_citation(chunk) else {
            continue;
        };

        if seen_urls.insert(citation.url.clone()) {
            citations.push(citation);
        }

        if citations.len() >= MAX_CITATIONS {
            break;
        }
    }

    citations
}

fn chunk_to_citation(chunk: &vertexai::model::GroundingChunk) -> Option<WebCitation> {
    if let Some(web) = chunk.web() {
        return citation_from_parts(web.title.as_deref(), web.uri.as_deref());
    }

    if let Some(retrieved_context) = chunk.retrieved_context() {
        return citation_from_parts(
            retrieved_context.title.as_deref(),
            retrieved_context.uri.as_deref(),
        );
    }

    if let Some(maps) = chunk.maps() {
        return citation_from_parts(maps.title.as_deref(), maps.uri.as_deref());
    }

    None
}

fn citation_from_parts(title: Option<&str>, url: Option<&str>) -> Option<WebCitation> {
    let url = url?.trim();
    if url.is_empty() {
        return None;
    }

    let title = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(url);

    Some(WebCitation {
        title: title.to_string(),
        url: url.to_string(),
    })
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }

    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use google_cloud_aiplatform_v1 as vertexai;
    use reili_core::knowledge::{WebSearchInput, WebSearchUserLocation};

    use super::{
        MAX_QUERY_CHARS, build_search_prompt, classify_api_error, parse_response, validate_query,
    };

    #[test]
    fn validates_query_length() {
        let query = "a".repeat(MAX_QUERY_CHARS + 1);

        let error = validate_query(&query).expect_err("query should be rejected");

        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn builds_prompt_with_timezone_context() {
        let prompt = build_search_prompt(&WebSearchInput {
            query: "google cloud status".to_string(),
            user_location: WebSearchUserLocation {
                timezone: "Asia/Tokyo".to_string(),
            },
        });

        assert!(prompt.contains("google cloud status"));
        assert!(prompt.contains("Asia/Tokyo"));
    }

    #[test]
    fn classifies_invalid_argument_as_client_error() {
        let result = classify_api_error("INVALID_ARGUMENT: unsupported tool");

        assert!(result.summary_text.contains("\"client_error\""));
    }

    #[test]
    fn parses_grounded_search_response() {
        let response = vertexai::model::GenerateContentResponse::new().set_candidates([
            vertexai::model::Candidate::new()
                .set_content(
                    vertexai::model::Content::new()
                        .set_role("model")
                        .set_parts([vertexai::model::Part::new()
                            .set_text("Google Cloud status is operational.")]),
                )
                .set_grounding_metadata(
                    vertexai::model::GroundingMetadata::new()
                        .set_web_search_queries([
                            "google cloud status today",
                            "google cloud incidents",
                        ])
                        .set_grounding_chunks([
                            vertexai::model::GroundingChunk::new().set_web(
                                vertexai::model::grounding_chunk::Web::new()
                                    .set_uri("https://status.cloud.google.com/")
                                    .set_title("Google Cloud Service Health"),
                            ),
                            vertexai::model::GroundingChunk::new().set_web(
                                vertexai::model::grounding_chunk::Web::new()
                                    .set_uri("https://cloud.google.com/blog")
                                    .set_title("Google Cloud Blog"),
                            ),
                        ])
                        .set_grounding_supports([vertexai::model::GroundingSupport::new()
                            .set_grounding_chunk_indices([1])]),
                ),
        ]);

        let result = parse_response(&response);

        assert_eq!(result.summary_text, "Google Cloud status is operational.");
        assert_eq!(result.searches.len(), 2);
        assert_eq!(result.searches[0].query, "google cloud status today");
        assert_eq!(result.searches[0].source_count, 2);
        assert_eq!(result.citations.len(), 2);
        assert_eq!(result.citations[0].url, "https://status.cloud.google.com/");
        assert_eq!(result.citations[1].url, "https://cloud.google.com/blog");
    }

    #[test]
    fn uses_prompt_feedback_when_response_is_blocked() {
        let response = vertexai::model::GenerateContentResponse::new().set_prompt_feedback(
            vertexai::model::generate_content_response::PromptFeedback::new()
                .set_block_reason_message("Blocked by policy"),
        );

        let result = parse_response(&response);

        assert!(result.summary_text.contains("\"client_error\""));
        assert!(result.summary_text.contains("Blocked by policy"));
    }
}
