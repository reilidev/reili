use std::sync::Arc;

use async_trait::async_trait;
use rig::tool::ToolDyn;

use crate::outbound::agents::connector::{
    ConnectorDescriptor, ConnectorFactory, ConnectorPrepareError, ConnectorPromptFact,
    PreparedConnector, SpecialistPromptContext,
};
use crate::outbound::agents::instructions_support::{
    append_configured_additional_system_prompt, reusable_notes_instruction,
    specialist_memory_context_instruction,
};
use crate::outbound::agents::tools::SearchPostsTool;
use crate::outbound::esa::EsaPostSearchPort;

const ESA_AGENT_NAME: &str = "investigate_esa";
const ESA_AGENT_DESCRIPTION: &str =
    "Delegates esa internal documentation, runbook, design note, team knowledge, and broader knowledge base search tasks.
When instructing this specialist, include the relevant background, context, and why the documentation search matters, not just the immediate keywords.";

/// Connector for the esa knowledge base, exposed through the domain port + hand-written tool.
pub struct EsaConnector {
    descriptor: ConnectorDescriptor,
    team_name: String,
    post_search_port: Arc<dyn EsaPostSearchPort>,
}

impl EsaConnector {
    #[must_use]
    pub fn new(team_name: String, post_search_port: Arc<dyn EsaPostSearchPort>) -> Self {
        Self {
            descriptor: ConnectorDescriptor {
                agent_name: ESA_AGENT_NAME.to_string(),
                agent_description: ESA_AGENT_DESCRIPTION.to_string(),
            },
            team_name,
            post_search_port,
        }
    }
}

#[async_trait]
impl ConnectorFactory for EsaConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        Ok(Arc::new(PreparedEsaConnector {
            descriptor: self.descriptor.clone(),
            team_name: self.team_name.clone(),
            post_search_port: Arc::clone(&self.post_search_port),
        }))
    }
}

struct PreparedEsaConnector {
    descriptor: ConnectorDescriptor,
    team_name: String,
    post_search_port: Arc<dyn EsaPostSearchPort>,
}

impl PreparedConnector for PreparedEsaConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    fn specialist_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        vec![Box::new(SearchPostsTool::new(Arc::clone(&self.post_search_port))) as Box<dyn ToolDyn>]
    }

    fn specialist_preamble(&self, context: &SpecialistPromptContext) -> String {
        build_esa_instructions(context, &self.team_name)
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

fn build_esa_instructions(context: &SpecialistPromptContext, team_name: &str) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();
    let memory_context_instruction = specialist_memory_context_instruction();

    append_configured_additional_system_prompt(
        format!(
            "You are an esa documentation search specialist with deep expertise in internal
knowledge discovery, including operational runbooks, incident notes, design
records, team processes, product specifications, onboarding guides, decision
logs, and general internal documentation.
Your role is to search esa team `{team_name}` and return concise, evidence-based
documentation findings that answer the current question and help the team find
relevant internal knowledge, whether the topic is operational, architectural,
procedural, or organizational.

Use {language} for all responses.

## Working style
Before entering a new major documentation search step, call report_progress.
The payload must be short and use the title and summary fields.
Work in a focused, question-driven way. Use search_posts to search esa posts
using esa query syntax. Prefer precise queries based on service names, alert
names, incident identifiers, repository names, owners, categories, tags,
feature names, project names, team names, and other domain keywords from the
task context.
Run tool calls in parallel whenever possible to reduce investigation latency.
Do not narrow your search to operational or investigation terms when the
request is asking for broader internal knowledge.

{memory_context_instruction}

## Evidence and output quality
Return concise findings rather than raw search output. Clearly distinguish
confirmed documentation facts, plausible inferences from docs, and remaining
unknowns. Include clickable esa URLs for all referenced posts whenever
available. Briefly summarize what you searched for and why, without dumping raw
tool arguments or raw tool output.

{reusable_notes_instruction}",
            team_name = team_name,
            language = context.language,
            memory_context_instruction = memory_context_instruction,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        context.additional_system_prompt.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::build_esa_instructions;
    use crate::outbound::agents::connector::SpecialistPromptContext;

    fn context(additional_system_prompt: Option<String>) -> SpecialistPromptContext {
        SpecialistPromptContext {
            language: "Japanese".to_string(),
            additional_system_prompt,
        }
    }

    #[test]
    fn appends_configured_additional_system_prompt() {
        let instructions = build_esa_instructions(
            &context(Some("Prefer runbook links first.".to_string())),
            "docs",
        );

        assert!(instructions.contains(
            "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
        ));
    }

    #[test]
    fn instructions_include_configured_team_name() {
        let instructions = build_esa_instructions(&context(None), "docs");

        assert!(instructions.contains("esa team `docs`"));
    }
}
