use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogMetricCatalogParams {
    pub from_epoch_sec: i64,
    pub tag_filter: Option<String>,
    pub limit: u32,
}

#[cfg_attr(any(test, feature = "test-support"), mockall::automock)]
#[async_trait]
pub trait DatadogMetricCatalogPort: Send + Sync {
    async fn list_metrics(
        &self,
        params: DatadogMetricCatalogParams,
    ) -> Result<Vec<String>, PortError>;
}
