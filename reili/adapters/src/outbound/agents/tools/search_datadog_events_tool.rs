use std::sync::Arc;

use reili_core::error::PortError;
use reili_core::investigation::InvestigationResources;
use reili_core::monitoring::datadog::DatadogEventSearchParams;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::datadog_tool_result_size_guard::serialize_datadog_tool_result_with_size_guard;
use super::datadog_tool_soft_error::to_datadog_tool_soft_error;
use super::tool_json::to_json_string;

#[derive(Clone)]
pub struct SearchDatadogEventsTool {
    resources: Arc<InvestigationResources>,
}

impl SearchDatadogEventsTool {
    pub fn new(resources: Arc<InvestigationResources>) -> Self {
        Self { resources }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDatadogEventsArgs {
    pub query: String,
    #[serde(default = "default_search_events_from")]
    pub from: String,
    #[serde(default = "default_search_events_to")]
    pub to: String,
    pub limit: u32,
}

fn default_search_events_from() -> String {
    "now-15m".to_string()
}

fn default_search_events_to() -> String {
    "now".to_string()
}

impl Tool for SearchDatadogEventsTool {
    const NAME: &'static str = "search_datadog_events";

    type Error = PortError;
    type Args = SearchDatadogEventsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Search Datadog events including GitHub integration events with a query and time range."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Datadog event search query"
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
                        "description": "Maximum number of events"
                    }
                },
                "required": ["query", "limit"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        match self
            .resources
            .event_search_port
            .search_events(DatadogEventSearchParams {
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
