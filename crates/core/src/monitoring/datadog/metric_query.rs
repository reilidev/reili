use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogMetricQueryPoint {
    pub time: String,
    pub v: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogMetricQueryResult {
    pub metric: Option<String>,
    pub unit: Option<String>,
    pub group_tags: Option<Vec<String>>,
    pub points: Vec<DatadogMetricQueryPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogMetricQueryParams {
    pub query: String,
    pub from: String,
    pub to: String,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait DatadogMetricQueryPort: Send + Sync {
    async fn query_metrics(
        &self,
        params: DatadogMetricQueryParams,
    ) -> Result<Vec<DatadogMetricQueryResult>, PortError>;
}
