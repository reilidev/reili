use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sre_shared::errors::PortError;
use sre_shared::ports::outbound::{DatadogLogSearchParams, InvestigationResources};

use super::datadog_tool_result_size_guard::serialize_datadog_tool_result_with_size_guard;
use super::datadog_tool_soft_error::to_datadog_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchDatadogLogsTool {
    resources: Arc<InvestigationResources>,
}

impl SearchDatadogLogsTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDatadogLogsArgs {
    pub query: String,
    #[serde(default = "default_search_logs_from")]
    pub from: String,
    #[serde(default = "default_search_logs_to")]
    pub to: String,
    pub limit: u32,
}

fn default_search_logs_from() -> String {
    "now-15m".to_string()
}

fn default_search_logs_to() -> String {
    "now".to_string()
}

impl Tool for SearchDatadogLogsTool {
    const NAME: &'static str = "search_datadog_logs";

    type Error = PortError;
    type Args = SearchDatadogLogsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Search Datadog logs with a query and time range. Returns recent log entries matching the query."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Datadog log search query"
                    },
                    "from": {
                        "type": "string",
                        "default": "now-15m",
                        "description": "Start time (date math or ISO string, e.g. now-15m or 2020-10-07T00:00:00+00:00)"
                    },
                    "to": {
                        "type": "string",
                        "default": "now",
                        "description": "End time (date math or ISO string, e.g. now or 2020-10-07T00:15:00+00:00)"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Maximum number of logs"
                    }
                },
                "required": ["query", "limit"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .resources
            .log_search_port
            .search_logs(DatadogLogSearchParams {
                query: args.query,
                from: args.from,
                to: args.to,
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
