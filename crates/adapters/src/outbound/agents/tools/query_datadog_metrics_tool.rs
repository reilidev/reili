use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::monitoring::datadog::DatadogMetricQueryParams;
use reili_core::task::TaskResources;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::datadog_tool_result_size_guard::serialize_datadog_tool_result_with_size_guard;
use super::datadog_tool_soft_error::to_datadog_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct QueryDatadogMetricsTool {
    resources: Arc<TaskResources>,
}

impl QueryDatadogMetricsTool {
    pub fn new(resources: Arc<TaskResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryDatadogMetricsArgs {
    pub query: String,
    pub from: String,
    pub to: String,
}

impl Tool for QueryDatadogMetricsTool {
    const NAME: &'static str = "query_datadog_metrics";

    type Error = PortError;
    type Args = QueryDatadogMetricsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Query Datadog timeseries metrics with a query and time range. Returns series with mapped time/value points."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Datadog metric query (e.g. avg:system.cpu{*})"
                    },
                    "from": {
                        "type": "string",
                        "description": "Start time in ISO 8601 format (e.g. 2020-10-07T00:00:00+00:00)"
                    },
                    "to": {
                        "type": "string",
                        "description": "End time in ISO 8601 format (e.g. 2020-10-07T00:15:00+00:00)"
                    }
                },
                "required": ["query", "from", "to"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .resources
            .metric_query_port
            .query_metrics(DatadogMetricQueryParams {
                query: args.query,
                from: args.from,
                to: args.to,
            })
            .await
        {
            Ok(results) => serialize_datadog_tool_result_with_size_guard(&results),
            Err(error) => {
                if let Some(soft_error) = to_datadog_tool_soft_error(&error) {
                    return to_json_string(&soft_error);
                }

                Err(error)
            }
        }
    }
}
