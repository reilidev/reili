use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact, PreparedConnector,
    ToolCatalogGroup,
};
use crate::outbound::agents::mcp::github::tools::{GitHubMcpToolset, connect_github_mcp_toolset};
use crate::outbound::github::GitHubMcpConfig;

/// Connector for GitHub repository context, exposed over the GitHub MCP server.
pub struct GitHubConnector {
    config: GitHubMcpConfig,
    scope_org: String,
}

impl GitHubConnector {
    #[must_use]
    pub fn new(config: GitHubMcpConfig, scope_org: String) -> Self {
        Self { config, scope_org }
    }
}

#[async_trait]
impl ConnectorFactory for GitHubConnector {
    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        let toolset = connect_github_mcp_toolset(&self.config, self.scope_org.clone())
            .await
            .map_err(ConnectorPrepareError::from_port_error)?;

        Ok(Arc::new(PreparedGitHubConnector {
            toolset,
            scope_org: self.scope_org.clone(),
        }))
    }
}

struct PreparedGitHubConnector {
    toolset: GitHubMcpToolset,
    scope_org: String,
}

impl PreparedConnector for PreparedGitHubConnector {
    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.sub_agent_tools()
    }

    fn spawn_tool_catalog(&self) -> ToolCatalogGroup {
        ToolCatalogGroup {
            source: "GitHub".to_string(),
            entries: self.toolset.sub_agent_catalog_entries(),
        }
    }

    fn spawn_guardrails(&self) -> Option<String> {
        Some(github_spawn_guardrails(&self.scope_org))
    }

    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        vec![ConnectorPromptFact {
            label: "GitHub Organization Scope".to_string(),
            value: self.scope_org.clone(),
        }]
    }
}

fn github_scope_rules(scope_org: &str) -> String {
    format!(
        "## Mandatory GitHub scope rules
Every `search_code`, `search_repositories`, `search_issues`, and
`search_pull_requests` call must include `org:{scope_org}`.
For `read_file`, `pull_request_read`, `actions_get`, `actions_list`,
`get_job_logs`, `get_dependabot_alert`, and `list_dependabot_alerts`, the
`owner` must be `{scope_org}`.
Never omit the org qualifier, switch owners, or access repositories outside
`{scope_org}`."
    )
}

fn github_spawn_guardrails(scope_org: &str) -> String {
    format!(
        "{scope_rules}

## GitHub usage notes
When exploring an unfamiliar repository, first read high-signal orientation
docs such as README and architecture documents before broad search to build a
working mental model, then choose focused follow-up queries instead of
scattered exploration. When searching code, prefer identifiers, service names,
config keys, endpoints, and dependency names over generic keywords. When
reading files, locate the relevant region first (for example with
`search_code`), then read just that range with `offset`/`limit`, widening only
if needed. When reviewing pull requests or issues, focus on recent changes,
intended behavior, rollout context, known risks, follow-up discussion, and
possible regressions. When reviewing Actions or Dependabot results, focus on
failing jobs, recent workflow regressions, vulnerable dependencies, severity,
fix guidance, and blast radius. Include the supporting GitHub URL for
referenced evidence whenever available.",
        scope_rules = github_scope_rules(scope_org),
    )
}

#[cfg(test)]
mod tests {
    use super::github_spawn_guardrails;

    #[test]
    fn guardrails_include_configured_scope_org() {
        let guardrails = github_spawn_guardrails("acme");

        assert!(guardrails.contains("org:acme"));
        assert!(guardrails.contains("the\n`owner` must be `acme`"));
    }

    #[test]
    fn guardrails_include_pr_and_actions_review_priorities() {
        let guardrails = github_spawn_guardrails("acme");

        assert!(guardrails.contains("rollout context, known risks"));
        assert!(guardrails.contains("vulnerable dependencies, severity"));
    }
}
