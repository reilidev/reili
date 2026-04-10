use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebCitation {
    pub title: String,
    pub url: String,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait WebSearchPort: Send + Sync {
    async fn search(&self, input: WebSearchInput) -> Result<WebSearchResult, PortError>;
}
