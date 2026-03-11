use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchInput {
    pub query: String,
    pub user_location: WebSearchUserLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchUserLocation {
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchResult {
    pub summary_text: String,
    pub citations: Vec<WebCitation>,
    pub searches: Vec<WebSearchExecution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebCitation {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchExecution {
    pub query: String,
    pub source_count: u32,
}

#[async_trait]
pub trait WebSearchPort: Send + Sync {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError>;
}
