use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogLogSearchResult {
    pub id: String,
    pub timestamp: String,
    pub service: Option<String>,
    pub status: Option<String>,
    pub message: Option<String>,
    pub attributes: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogLogSearchParams {
    pub query: String,
    pub from: String,
    pub to: String,
    pub limit: u32,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait DatadogLogSearchPort: Send + Sync {
    async fn search_logs(
        &self,
        params: DatadogLogSearchParams,
    ) -> Result<Vec<DatadogLogSearchResult>, PortError>;
}

#[cfg(test)]
mod tests {
    use super::DatadogLogSearchResult;
    use serde_json::json;

    #[test]
    fn serializes_and_deserializes_log_search_result() {
        let value = DatadogLogSearchResult {
            id: "log-1".to_string(),
            timestamp: "2026-03-04T00:00:00Z".to_string(),
            service: Some("api".to_string()),
            status: Some("error".to_string()),
            message: Some("failed".to_string()),
            attributes: Some(json!({"env": "prod"})),
        };

        let json = serde_json::to_string(&value).expect("serialize datadog log search result");
        let restored: DatadogLogSearchResult =
            serde_json::from_str(&json).expect("deserialize datadog log search result");

        assert_eq!(restored, value);
    }
}
