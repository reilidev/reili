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
use crate::outbound::agents::mcp::datadog::tools::{
    DatadogMcpToolset, connect_datadog_mcp_toolset,
};
use crate::outbound::datadog::DatadogMcpToolConfig;

const DATADOG_AGENT_NAME: &str = "investigate_datadog";
const DATADOG_AGENT_DESCRIPTION: &str =
    "Delegates Datadog observability and security investigation tasks.
This tool is designed to be split into scopes and used in parallel.
When instructing this specialist, include the relevant background, context, and why the investigation matters, not just the immediate question.";

const DEFAULT_DATADOG_SITE: &str = "datadoghq.com";

/// Connector for Datadog telemetry, exposed over the Datadog MCP server.
pub struct DatadogConnector {
    descriptor: ConnectorDescriptor,
    config: DatadogMcpToolConfig,
}

impl DatadogConnector {
    #[must_use]
    pub fn new(config: DatadogMcpToolConfig) -> Self {
        Self {
            descriptor: ConnectorDescriptor {
                agent_name: DATADOG_AGENT_NAME.to_string(),
                agent_description: DATADOG_AGENT_DESCRIPTION.to_string(),
            },
            config,
        }
    }
}

#[async_trait]
impl ConnectorFactory for DatadogConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    async fn prepare(&self) -> Result<Arc<dyn PreparedConnector>, ConnectorPrepareError> {
        let toolset = connect_datadog_mcp_toolset(&self.config)
            .await
            .map_err(ConnectorPrepareError::from_port_error)?;

        Ok(Arc::new(PreparedDatadogConnector {
            descriptor: self.descriptor.clone(),
            toolset,
            site: self.config.site.clone(),
        }))
    }
}

struct PreparedDatadogConnector {
    descriptor: ConnectorDescriptor,
    toolset: DatadogMcpToolset,
    site: String,
}

impl PreparedConnector for PreparedDatadogConnector {
    fn descriptor(&self) -> &ConnectorDescriptor {
        &self.descriptor
    }

    fn specialist_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.specialist_tools()
    }

    fn lead_tools(&self) -> Vec<Box<dyn ToolDyn>> {
        self.toolset.lead_tools()
    }

    fn specialist_preamble(&self, context: &SpecialistPromptContext) -> String {
        build_datadog_instructions(context)
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

fn build_datadog_instructions(context: &SpecialistPromptContext) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();
    let memory_context_instruction = specialist_memory_context_instruction();

    append_configured_additional_system_prompt(
        format!(
        "You are a Datadog investigation specialist with deep expertise in production reliability, observability, failure analysis, operational diagnostics, and security investigation.
Your role is to investigate Datadog evidence across logs, metrics, events, dashboards, Synthetic tests, and any available Datadog security tools, and return concise, evidence-based findings that support safe and reliable operational decisions.

Use {language} for all responses.

## Investigation approach
Work in a hypothesis-driven way. Start by narrowing the service, timeframe, and current working hypothesis, then use only the Datadog tools needed to test that hypothesis or answer the current question. Prefer focused investigation over broad data collection.
Run tool calls in parallel whenever possible to reduce investigation latency.
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.

{memory_context_instruction}

## Output expectations
Prioritize the most operationally relevant questions first: customer impact, affected scope, onset time, likely trigger, severity, and whether the issue is ongoing.
Return concise, high-signal findings rather than raw tool output. Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns. Avoid overstating conclusions, and state uncertainty explicitly when evidence is partial, indirect, or conflicting.
Include clickable Datadog links for all referenced evidence whenever available. Briefly summarize the investigation trail so another engineer can follow what you checked, why you checked it, and what each step established, without dumping raw tool arguments or raw tool output.

{reusable_notes_instruction}"
            ,
            language = context.language,
            memory_context_instruction = memory_context_instruction,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        context.additional_system_prompt.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_DATADOG_SITE, build_datadog_instructions};
    use crate::outbound::agents::connector::SpecialistPromptContext;

    fn context(additional_system_prompt: Option<String>) -> SpecialistPromptContext {
        SpecialistPromptContext {
            language: "Japanese".to_string(),
            additional_system_prompt,
        }
    }

    #[test]
    fn appends_configured_additional_system_prompt() {
        let instructions = build_datadog_instructions(&context(Some(
            "Prefer runbook links first.\nState uncertainty explicitly.".to_string(),
        )));

        assert!(instructions.contains(
            "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
        ));
        assert!(instructions.contains("State uncertainty explicitly."));
    }

    #[test]
    fn instructions_use_configured_language() {
        let instructions = build_datadog_instructions(&context(None));

        assert!(instructions.contains("Use Japanese for all responses."));
    }

    #[test]
    fn default_site_is_datadoghq() {
        assert_eq!(DEFAULT_DATADOG_SITE, "datadoghq.com");
    }
}
