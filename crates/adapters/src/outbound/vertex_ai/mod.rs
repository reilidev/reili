mod vertex_ai_web_search_adapter;

pub use vertex_ai_web_search_adapter::{VertexAiWebSearchAdapter, VertexAiWebSearchAdapterConfig};

pub const ANTHROPIC_PUBLISHER: &str = "anthropic";
pub const ANTHROPIC_VERTEX_VERSION: &str = "vertex-2023-10-16";

pub fn vertex_ai_base_url(location: &str) -> String {
    if location == "global" {
        "https://aiplatform.googleapis.com".to_string()
    } else {
        format!("https://{location}-aiplatform.googleapis.com")
    }
}
