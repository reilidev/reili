use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::knowledge::{WebSearchInput, WebSearchPort, WebSearchResult};

const MAX_QUERY_CHARS: usize = 500;

pub struct BedrockWebSearchAdapterConfig {
    pub model_id: String,
}

pub struct BedrockWebSearchAdapter {
    model_id: String,
}

impl BedrockWebSearchAdapter {
    pub fn new(config: BedrockWebSearchAdapterConfig) -> Self {
        Self {
            model_id: config.model_id,
        }
    }
}

#[async_trait]
impl WebSearchPort for BedrockWebSearchAdapter {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError> {
        if input.query.is_empty() {
            return Err(PortError::new("Web search query must not be empty"));
        }
        if input.query.chars().count() > MAX_QUERY_CHARS {
            return Err(PortError::new(format!(
                "Web search query exceeds {MAX_QUERY_CHARS} characters"
            )));
        }

        Ok(WebSearchResult {
            summary_text: format!(
                "{{\"type\":\"capability_unavailable\",\"message\":\"Web search is not available for AWS Bedrock model {}\"}}",
                self.model_id
            ),
            citations: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use reili_core::knowledge::{WebSearchInput, WebSearchPort, WebSearchUserLocation};

    use super::{BedrockWebSearchAdapter, BedrockWebSearchAdapterConfig};

    #[tokio::test]
    async fn returns_capability_unavailable_response() {
        let adapter = BedrockWebSearchAdapter::new(BedrockWebSearchAdapterConfig {
            model_id: "anthropic.claude-3-7-sonnet-20250219-v1:0".to_string(),
        });

        let result = adapter
            .search(WebSearchInput {
                query: "aws status".to_string(),
                user_location: WebSearchUserLocation {
                    timezone: "Asia/Tokyo".to_string(),
                },
            })
            .await
            .expect("search should succeed");

        assert!(result.summary_text.contains("capability_unavailable"));
        assert!(result.citations.is_empty());
    }
}
