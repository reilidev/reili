use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact, PreparedConnector,
    ToolCatalogGroup,
};
use crate::outbound::agents::mcp::datadog::tools::{
    DatadogMcpToolset, connect_datadog_mcp_toolset,
};
use crate::outbound::datadog::DatadogMcpToolConfig;

const DEFAULT_DATADOG_SITE: &str = "datadoghq.com";

const DATADOG_SPAWN_GUARDRAILS: &str = "## Datadog usage notes
Work in a hypothesis-driven way: narrow the service, timeframe, and working
hypothesis first, then use only the Datadog tools needed to test it. Prefer
focused queries over broad data collection. Prioritize the most operationally
relevant questions first: customer impact, affected scope, onset time, likely
trigger, severity, and whether the issue is ongoing. Include clickable Datadog
links for referenced evidence whenever available.";

/// Connector for Datadog telemetry, exposed over the Datadog MCP server.
pub struct DatadogConnector {
    config: DatadogMcpToolConfig,
}

impl DatadogConnector {
    #[must_use]
    pub fn new(config: DatadogMcpToolConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ConnectorFactory for DatadogConnector {
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        let toolset = connect_datadog_mcp_toolset(&self.config)
            .await
            .map_err(ConnectorPrepareError::from_port_error)?;

        Ok(Arc::new(PreparedDatadogConnector {
            toolset,
            site: self.config.site.clone(),
        }))
    }
}

struct PreparedDatadogConnector {
    toolset: DatadogMcpToolset,
    site: String,
}

impl PreparedConnector for PreparedDatadogConnector {
    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.sub_agent_tools()
    }

    fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.lead_tools()
    }

    fn spawn_tool_catalog(&self) -> ToolCatalogGroup {
        ToolCatalogGroup {
            source: "Datadog".to_string(),
            entries: self.toolset.sub_agent_catalog_entries(),
        }
    }

    fn spawn_guardrails(&self) -> Option<String> {
        Some(DATADOG_SPAWN_GUARDRAILS.to_string())
    }

    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        let site = if self.site.is_empty() {
            DEFAULT_DATADOG_SITE
        } else {
            self.site.as_str()
        };

        vec![ConnectorPromptFact {
            label: "Datadog Site".to_string(),
            value: site.to_string(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::{DATADOG_SPAWN_GUARDRAILS, DEFAULT_DATADOG_SITE};

    #[test]
    fn default_site_is_datadoghq() {
        assert_eq!(DEFAULT_DATADOG_SITE, "datadoghq.com");
    }

    #[test]
    fn guardrails_mention_hypothesis_driven_investigation_and_operational_priorities() {
        assert!(DATADOG_SPAWN_GUARDRAILS.contains("hypothesis-driven"));
        assert!(DATADOG_SPAWN_GUARDRAILS.contains("customer impact"));
    }
}
