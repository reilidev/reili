use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorDescriptor, ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact,
    PreparedConnector, SubAgentPromptContext,
};
use crate::outbound::agents::instructions_support::{
    append_configured_additional_system_prompt, reusable_notes_instruction,
    sub_agent_memory_context_instruction,
};
use crate::outbound::agents::mcp::github::tools::{GitHubMcpToolset, connect_github_mcp_toolset};
use crate::outbound::github::GitHubMcpConfig;

const GITHUB_AGENT_NAME: &str = "investigate_github";
const GITHUB_AGENT_DESCRIPTION: &str =
    "Delegates GitHub repository, code, pull request, Actions, and Dependabot investigation tasks.
This tool is designed to be split into scopes and used in parallel.
When instructing this sub-agent, include the relevant background, context, and why the investigation matters, not just the immediate question.";

/// Connector for GitHub repository context, exposed over the GitHub MCP server.
pub struct GitHubConnector {
    descriptor: ConnectorDescriptor,
    config: GitHubMcpConfig,
    scope_org: String,
}

impl GitHubConnector {
    #[must_use]
    pub fn new(config: GitHubMcpConfig, scope_org: String) -> Self {
        Self {
            descriptor: ConnectorDescriptor {
                agent_name: GITHUB_AGENT_NAME.to_string(),
                agent_description: GITHUB_AGENT_DESCRIPTION.to_string(),
            },
            config,
            scope_org,
        }
    }
}

#[async_trait]
impl ConnectorFactory for GitHubConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        let toolset = connect_github_mcp_toolset(&self.config, self.scope_org.clone())
            .await
            .map_err(ConnectorPrepareError::from_port_error)?;

        Ok(Arc::new(PreparedGitHubConnector {
            descriptor: self.descriptor.clone(),
            toolset,
            scope_org: self.scope_org.clone(),
        }))
    }
}

struct PreparedGitHubConnector {
    descriptor: ConnectorDescriptor,
    toolset: GitHubMcpToolset,
    scope_org: String,
}

impl PreparedConnector for PreparedGitHubConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    fn sub_agent_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.sub_agent_tools()
    }

    fn sub_agent_preamble(&self, context: &SubAgentPromptContext) -> String {
        build_github_instructions(context, &self.scope_org)
    }

    fn prompt_facts(&self) -> Vec<ConnectorPromptFact> {
        vec![ConnectorPromptFact {
            label: "GitHub Organization Scope".to_string(),
            value: self.scope_org.clone(),
        }]
    }
}

fn build_github_instructions(context: &SubAgentPromptContext, scope_org: &str) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();
    let memory_context_instruction = sub_agent_memory_context_instruction();

    append_configured_additional_system_prompt(
        format!(
            "You are a GitHub sub-agent with deep expertise in software
development, repository analysis, and change investigation. Your role is to
use GitHub evidence to clarify system structure, ownership, code behavior,
recent changes, pull request context, and other repository facts that matter
for correct and reliable engineering decisions.

Use {language} for all responses.

## Working style
Before entering a new major investigation step, call report_progress. The
payload must be short and use the title and summary fields.
Work in a focused, question-driven way. Use the available GitHub MCP tools to
search code, repositories, issues, pull requests, repository files, GitHub
Actions workflows and job logs, and Dependabot alerts, but only to the extent
needed to answer the current question or test the current hypothesis. Run tool
calls in parallel whenever possible to reduce investigation latency.

{memory_context_instruction}

## Mandatory scope rules
Every `search_code`, `search_repositories`, `search_issues`, and
`search_pull_requests` call must include `org:{github_scope_org}`.
For `read_file`, `pull_request_read`, `actions_get`, `actions_list`,
`get_job_logs`, `get_dependabot_alert`, and `list_dependabot_alerts`, the
`owner` must be `{github_scope_org}`.
Never omit the org qualifier, switch owners, or access repositories outside
`{github_scope_org}`.

## What to prioritize
Prioritize repository evidence that helps explain system behavior, ownership,
recent changes, deployment context, configuration, architecture, interfaces,
CI failures, dependency risk, and likely user impact.
When starting exploration for an unfamiliar repository, first read high-signal
orientation docs such as README, architecture/design documents, and key
technical documentation to build a working mental model before broad search.
Use that model to choose focused follow-up queries and avoid scattered
exploration.
When searching code, prefer identifiers, service names, config keys, endpoints,
dependency names, and domain-relevant paths over generic keywords. When
reviewing pull requests or issues, focus on recent changes, intended behavior,
rollout context, known risks, follow-up discussion, and possible regressions.
When reviewing Actions or Dependabot results, focus on failing jobs, recent
workflow regressions, vulnerable dependencies, severity, fix guidance, and
blast radius.
When reading files, use `read_file` and extract only the minimum necessary
context needed to answer accurately. Locate the relevant region first (for
example with `search_code`), then read just that range with `offset`/`limit`,
widening only if needed. Prefer concise summaries over large excerpts.

## Evidence and output quality
Return concise, evidence-based findings rather than raw search output. Clearly
distinguish confirmed facts, plausible inferences, and remaining unknowns.
Avoid overstating conclusions when repository evidence is partial, indirect, or
ambiguous.
Whenever you reference GitHub evidence, include the supporting GitHub URL as a
clickable link whenever available. Briefly summarize the investigation trail so
another engineer can follow what you checked, why you checked it, and what each
step established, without dumping raw tool arguments or raw tool output.

{reusable_notes_instruction}",
            github_scope_org = scope_org,
            language = context.language,
            memory_context_instruction = memory_context_instruction,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        context.additional_system_prompt.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::build_github_instructions;
    use crate::outbound::agents::connector::SubAgentPromptContext;

    fn context(additional_system_prompt: Option<String>) -> SubAgentPromptContext {
        SubAgentPromptContext {
            language: "Japanese".to_string(),
            additional_system_prompt,
        }
    }

    #[test]
    fn appends_configured_additional_system_prompt() {
        let instructions = build_github_instructions(
            &context(Some("Prefer runbook links first.".to_string())),
            "acme",
        );

        assert!(instructions.contains(
            "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
        ));
    }

    #[test]
    fn instructions_include_configured_scope_org() {
        let instructions = build_github_instructions(&context(None), "acme");

        assert!(instructions.contains("org:acme"));
        assert!(instructions.contains("the\n`owner` must be `acme`"));
    }
}
