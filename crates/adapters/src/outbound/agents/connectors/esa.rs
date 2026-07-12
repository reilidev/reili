use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::Tool;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact, PreparedConnector,
    ToolCatalogEntry, ToolCatalogGroup,
};
use crate::outbound::agents::tools::SearchPostsTool;
use crate::outbound::esa::EsaPostSearchPort;

/// Connector for the esa knowledge base, exposed through the domain port + hand-written tool.
pub struct EsaConnector {
    team_name: String,
    post_search_port: Arc<dyn EsaPostSearchPort>,
}

impl EsaConnector {
    #[must_use]
    pub fn new(team_name: String, post_search_port: Arc<dyn EsaPostSearchPort>) -> Self {
        Self {
            team_name,
            post_search_port,
        }
    }
}

#[async_trait]
impl ConnectorFactory for EsaConnector {
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        Ok(Arc::new(PreparedEsaConnector {
            team_name: self.team_name.clone(),
            post_search_port: Arc::clone(&self.post_search_port),
        }))
    }
}

struct PreparedEsaConnector {
    team_name: String,
    post_search_port: Arc<dyn EsaPostSearchPort>,
}

impl PreparedConnector for PreparedEsaConnector {
    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        vec![Box::new(SearchPostsTool::new(Arc::clone(&self.post_search_port))) as Box<dyn ToolDyn>]
    }

    fn spawn_tool_catalog(&self) -> ToolCatalogGroup {
        ToolCatalogGroup {
            source: "esa".to_string(),
            entries: vec![ToolCatalogEntry::new(
                SearchPostsTool::NAME,
                "Search esa internal documentation posts using esa query syntax.",
            )],
        }
    }

    fn spawn_guardrails(&self) -> Option<String> {
        Some(esa_spawn_guardrails(&self.team_name))
    }

    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        let team_name = self.team_name.trim();
        if team_name.is_empty() {
            return Vec::new();
        }

        vec![ConnectorPromptFact {
            label: "esa Team".to_string(),
            value: team_name.to_string(),
        }]
    }
}

fn esa_spawn_guardrails(team_name: &str) -> String {
    format!(
        "## esa usage notes
`search_posts` searches the esa team `{team_name}` using esa query syntax.
Prefer precise queries based on service names, alert names, incident
identifiers, repository names, categories, tags, and other domain keywords. Do
not narrow your search to operational or investigation terms when the request
is asking for broader internal knowledge. Include clickable esa URLs for
referenced posts whenever available.",
        team_name = team_name,
    )
}

#[cfg(test)]
mod tests {
    use super::esa_spawn_guardrails;

    #[test]
    fn guardrails_include_configured_team_name() {
        let guardrails = esa_spawn_guardrails("docs");

        assert!(guardrails.contains("esa team `docs`"));
    }

    #[test]
    fn guardrails_mention_broader_knowledge_requests() {
        let guardrails = esa_spawn_guardrails("docs");

        assert!(guardrails.contains("broader internal knowledge"));
    }
}
