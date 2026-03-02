use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogEventSearchResult {
    pub id: String,
    pub timestamp: String,
    pub source: Option<String>,
    pub status: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogEventSearchParams {
    pub query: String,
    pub from: String,
    pub to: String,
    pub limit: u32,
}

#[async_trait]
pub trait DatadogEventSearchPort: Send + Sync {
    async fn search_events(
        &self,
        params: DatadogEventSearchParams,
    ) -> Result<Vec<DatadogEventSearchResult>, PortError>;
}
