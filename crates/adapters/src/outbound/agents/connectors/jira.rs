use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact, PreparedConnector,
    ToolCatalogGroup,
};
use crate::outbound::agents::mcp::jira::tools::{JiraMcpToolset, connect_jira_mcp_toolset};
use crate::outbound::jira::JiraMcpConfig;

/// Connector for JIRA ticket search and reference, exposed over the Atlassian Rovo MCP server.
pub struct JiraConnector {
    config: JiraMcpConfig,
}

impl JiraConnector {
    #[must_use]
    pub fn new(config: JiraMcpConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ConnectorFactory for JiraConnector {
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        let toolset = connect_jira_mcp_toolset(&self.config)
            .await
            .map_err(ConnectorPrepareError::from_port_error)?;

        Ok(Arc::new(PreparedJiraConnector {
            toolset,
            site: self.config.site.clone(),
        }))
    }
}

struct PreparedJiraConnector {
    toolset: JiraMcpToolset,
    site: String,
}

impl PreparedConnector for PreparedJiraConnector {
    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.sub_agent_tools()
    }

    fn spawn_tool_catalog(&self) -> ToolCatalogGroup {
        ToolCatalogGroup {
            source: "JIRA".to_string(),
            entries: self.toolset.sub_agent_catalog_entries(),
        }
    }

    fn spawn_guardrails(&self) -> Option<String> {
        Some(jira_spawn_guardrails(&self.site))
    }

    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        jira_site_prompt_facts(&self.site)
    }
}

fn jira_site_prompt_facts(site: &str) -> Vec<ConnectorPromptFact> {
    let site = site.trim();
    if site.is_empty() {
        return Vec::new();
    }

    vec![ConnectorPromptFact {
        label: "JIRA Site".to_string(),
        value: site.to_string(),
    }]
}

fn jira_spawn_guardrails(site: &str) -> String {
    format!(
        "## JIRA usage notes
Prefer precise JQL built from ticket identifiers, component names, labels,
status, and reporter/assignee over broad unscoped scans. Use
`getJiraIssue` to read an issue's full detail (description, status,
assignee, comments, and issue links) once you have narrowed to specific
tickets via search. There is no separate comments tool: comments are part
of an issue's fields, so if the default `getJiraIssue` response omits
recent comments, re-request the issue and explicitly include the
`comment` field (via the tool's `fields`/`expand` parameter) to read the
full comment thread. Comments often carry the most useful investigation
context (root cause notes, follow-ups, resolution rationale), so check
them whenever a ticket's description alone does not explain the current
state. Include a clickable JIRA URL
(`https://{site}/browse/{{ISSUE-KEY}}`) whenever citing evidence from a
ticket."
    )
}

#[cfg(test)]
mod tests {
    use super::{jira_site_prompt_facts, jira_spawn_guardrails};

    #[test]
    fn guardrails_include_browse_url_template_for_configured_site() {
        let guardrails = jira_spawn_guardrails("acme.atlassian.net");

        assert!(guardrails.contains("https://acme.atlassian.net/browse/"));
    }

    #[test]
    fn guardrails_explain_how_to_read_comments_via_get_jira_issue() {
        let guardrails = jira_spawn_guardrails("acme.atlassian.net");

        assert!(guardrails.contains("no separate comments tool"));
        assert!(guardrails.contains("`comment` field"));
    }

    #[test]
    fn prompt_facts_expose_configured_jira_site() {
        let facts = jira_site_prompt_facts("acme.atlassian.net");

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].label, "JIRA Site");
        assert_eq!(facts[0].value, "acme.atlassian.net");
    }

    #[test]
    fn prompt_facts_are_empty_for_blank_site() {
        assert!(jira_site_prompt_facts("   ").is_empty());
    }
}
