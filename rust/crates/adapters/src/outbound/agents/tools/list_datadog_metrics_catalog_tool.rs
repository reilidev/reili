use std::sync::Arc;

use chrono::Utc;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{DatadogMetricCatalogParams, InvestigationResources};

use super::datadog_tool_soft_error::to_datadog_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct ListDatadogMetricsCatalogTool {
    resources: Arc<InvestigationResources>,
}

impl ListDatadogMetricsCatalogTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDatadogMetricsCatalogArgs {
    #[serde(default = "default_lookback_hours")]
    pub lookback_hours: u32,
    #[serde(default)]
    pub tag_filter: String,
    #[serde(default = "default_metrics_catalog_limit")]
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListMetricsCatalogToolResult {
    total: u64,
    metrics: Vec<String>,
}

fn default_lookback_hours() -> u32 {
    24
}

fn default_metrics_catalog_limit() -> u32 {
    100
}

impl Tool for ListDatadogMetricsCatalogTool {
    const NAME: &'static str = "list_datadog_metrics_catalog";

    type Error = PortError;
    type Args = ListDatadogMetricsCatalogArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "List available Datadog metrics for a recent time window. Returns metric names, prefix counts, and representative examples for environment discovery."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "lookbackHours": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 720,
                        "default": 24,
                        "description": "How many hours back to scan for active metrics"
                    },
                    "tagFilter": {
                        "type": "string",
                        "default": "",
                        "description": "Optional Datadog tag filter expression (e.g. env:prod, service:system-*)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "default": 100,
                        "description": "Maximum number of unique metric names to return"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let now_epoch_sec = Utc::now().timestamp();
        let from_epoch_sec = now_epoch_sec - i64::from(args.lookback_hours) * 60 * 60;
        let tag_filter = if args.tag_filter.trim().is_empty() {
            None
        } else {
            Some(args.tag_filter)
        };

        match self
            .resources
            .metric_catalog_port
            .list_metrics(DatadogMetricCatalogParams {
                from_epoch_sec,
                tag_filter,
                limit: args.limit,
            })
            .await
        {
            Ok(metrics) => to_json_string(&ListMetricsCatalogToolResult {
                total: u64::try_from(metrics.len()).unwrap_or(u64::MAX),
                metrics,
            }),
            Err(error) => {
                if let Some(soft_error) = to_datadog_tool_soft_error(&error) {
                    return to_json_string(&soft_error);
                }

                Err(error)
            }
        }
    }
}
