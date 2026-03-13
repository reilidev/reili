use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::PortError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogLogAggregateBucket {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogLogAggregateParams {
    pub query: String,
    pub from: String,
    pub to: String,
    pub facet: String,
    pub limit: u32,
}

#[async_trait]
pub trait DatadogLogAggregatePort: Send + Sync {
    async fn aggregate_by_facet(
        &self,
        params: DatadogLogAggregateParams,
    ) -> Result<Vec<DatadogLogAggregateBucket>, PortError>;
}
