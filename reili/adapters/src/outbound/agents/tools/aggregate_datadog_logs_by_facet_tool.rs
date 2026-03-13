use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use reili_shared::errors::PortError;
use reili_shared::ports::outbound::{DatadogLogAggregateParams, InvestigationResources};

use super::datadog_tool_result_size_guard::serialize_datadog_tool_result_with_size_guard;
use super::datadog_tool_soft_error::to_datadog_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct AggregateDatadogLogsByFacetTool {
    resources: Arc<InvestigationResources>,
}

impl AggregateDatadogLogsByFacetTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateDatadogLogsByFacetArgs {
    #[serde(default = "default_aggregate_query")]
    pub query: String,
    #[serde(default = "default_aggregate_from")]
    pub from: String,
    #[serde(default = "default_aggregate_to")]
    pub to: String,
    #[serde(default = "default_aggregate_facet")]
    pub facet: String,
    #[serde(default = "default_aggregate_limit")]
    pub limit: u32,
}

fn default_aggregate_query() -> String {
    "*".to_string()
}

fn default_aggregate_from() -> String {
    "now-30m".to_string()
}

fn default_aggregate_to() -> String {
    "now".to_string()
}

fn default_aggregate_facet() -> String {
    "service".to_string()
}

fn default_aggregate_limit() -> u32 {
    20
}

impl Tool for AggregateDatadogLogsByFacetTool {
    const NAME: &'static str = "aggregate_datadog_logs_by_facet";

    type Error = PortError;
    type Args = AggregateDatadogLogsByFacetArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Aggregate Datadog logs by facet and return top buckets by count. Use this to discover active services early in an investigation."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "default": "*",
                        "description": "Datadog log search query used before aggregation"
                    },
                    "from": {
                        "type": "string",
                        "default": "now-30m",
                        "description": "Start time (date math or ISO string, e.g. now-30m)"
                    },
                    "to": {
                        "type": "string",
                        "default": "now",
                        "description": "End time (date math or ISO string, e.g. now)"
                    },
                    "facet": {
                        "type": "string",
                        "default": "service",
                        "description": "Facet name used for aggregation. Defaults to service."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 20,
                        "description": "Maximum number of buckets to return"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .resources
            .log_aggregate_port
            .aggregate_by_facet(DatadogLogAggregateParams {
                query: args.query,
                from: args.from,
                to: args.to,
                facet: args.facet,
                limit: args.limit,
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
