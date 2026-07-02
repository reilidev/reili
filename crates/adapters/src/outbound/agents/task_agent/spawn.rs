//! Assembly pieces for dynamically spawned sub-agents: the fixed preamble frame composed around
//! the lead-generated mission, the compact tool catalog shown to the lead, and the resolution of
//! a lead's tool selection into concrete tool instances and per-source guardrails.

use std::collections::HashSet;
use std::sync::Arc;

use reili_core::task::TaskResources;
use rig::tool::{Tool, ToolDyn};

use crate::outbound::agents::connector::{PreparedConnector, ToolCatalogEntry, ToolCatalogGroup};
use crate::outbound::agents::instructions_support::{
    append_configured_additional_system_prompt, reusable_notes_instruction,
    sub_agent_memory_context_instruction,
};
use crate::outbound::agents::tools::SearchWebTool;

pub(super) struct ComposeSpawnedPreambleInput<'a> {
    pub(super) language: &'a str,
    pub(super) additional_system_prompt: Option<&'a str>,
    pub(super) lead_instructions: &'a str,
    pub(super) guardrails: &'a [String],
}

/// Composes the full preamble of a spawned sub-agent: the fixed frame (working style, memory
/// handling, output quality, configured additional prompt) around the lead-generated mission and
/// the guardrails of the tool sources involved.
pub(super) fn compose_spawned_sub_agent_preamble(
    input: &ComposeSpawnedPreambleInput<'_>,
) -> String {
    let memory_context_instruction = sub_agent_memory_context_instruction();
    let reusable_notes_instruction = reusable_notes_instruction();
    let guardrails_section = if input.guardrails.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", input.guardrails.join("\n\n"))
    };

    append_configured_additional_system_prompt(
        format!(
            "You are a focused sub-agent spawned by the lead agent for a single
delegated task. Your mission below was composed by the lead agent for this
specific delegation.

Use {language} for all responses.

## Working style
Before entering a new major step, call report_progress. The
payload must be short and use the title and summary fields.
Work in a focused, question-driven way, using only the tools you have been
given. Run tool calls in parallel whenever possible to reduce latency.

{memory_context_instruction}

## Mission
{lead_instructions}{guardrails_section}

## Evidence and output quality
Return concise, evidence-based findings rather than raw tool output. Clearly
distinguish confirmed facts, plausible inferences, and remaining unknowns.
Avoid overstating conclusions when evidence is partial, indirect, or
ambiguous. Include supporting URLs as clickable links whenever available.
Briefly summarize what you did so another engineer can follow what you
checked, why you checked it, and what each step established, without dumping
raw tool arguments or raw tool output.

{reusable_notes_instruction}",
            language = input.language,
            memory_context_instruction = memory_context_instruction,
            lead_instructions = input.lead_instructions,
            guardrails_section = guardrails_section,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt,
    )
}

const SEARCH_WEB_CATALOG_SUMMARY: &str =
    "Search the public web for vendor outages, status pages, documentation, and error messages.";

/// Catalog group for tools supplied by the task agent itself rather than a connector.
pub(super) fn built_in_spawn_catalog_group() -> ToolCatalogGroup {
    ToolCatalogGroup {
        source: "General".to_string(),
        entries: vec![ToolCatalogEntry::new(
            SearchWebTool::NAME,
            SEARCH_WEB_CATALOG_SUMMARY,
        )],
    }
}

/// Full spawn catalog: every connector's group in registration order, then the built-in group.
pub(super) fn spawn_tool_catalog_groups(
    prepared_connectors: &[Arc<dyn PreparedConnector>],
) -> Vec<ToolCatalogGroup> {
    prepared_connectors
        .iter()
        .map(|prepared| prepared.spawn_tool_catalog())
        .chain(std::iter::once(built_in_spawn_catalog_group()))
        .collect()
}

/// Renders the compact catalog embedded in the lead system prompt: one line per tool, grouped by
/// source. Full tool schemas are only injected into the spawned sub-agent.
pub(super) fn render_spawn_tool_catalog(groups: &[ToolCatalogGroup]) -> String {
    groups
        .iter()
        .filter(|group| !group.entries.is_empty())
        .map(|group| {
            let entries = group
                .entries
                .iter()
                .map(|entry| format!("- {}: {}", entry.name, entry.summary))
                .collect::<Vec<_>>()
                .join("\n");
            format!("## {}\n{}", group.source, entries)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Flattened, order-preserving list of selectable tool names across all catalog groups.
pub(super) fn spawn_catalog_tool_names(groups: &[ToolCatalogGroup]) -> Vec<String> {
    let mut seen = HashSet::new();
    groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .filter(|entry| seen.insert(entry.name.clone()))
        .map(|entry| entry.name.clone())
        .collect()
}

pub(super) struct ResolveSpawnSelectionInput<'a> {
    pub(super) prepared_connectors: &'a [Arc<dyn PreparedConnector>],
    pub(super) resources: &'a Arc<TaskResources>,
    pub(super) tool_names: &'a [String],
}

pub(super) struct ResolvedSpawnSelection {
    pub(super) tools: Vec<Box<dyn ToolDyn>>,
    pub(super) guardrails: Vec<String>,
}

/// Resolves a validated tool selection into concrete tool instances, collecting the guardrails of
/// every source that contributes at least one tool. Names were validated against the catalog at
/// the spawn boundary, so unknown names are simply skipped here.
pub(super) fn resolve_spawn_selection(
    input: &ResolveSpawnSelectionInput<'_>,
) -> ResolvedSpawnSelection {
    let requested: HashSet<&str> = input.tool_names.iter().map(String::as_str).collect();
    let mut tools: Vec<Box<dyn ToolDyn>> = Vec::new();
    let mut guardrails = Vec::new();

    for prepared in input.prepared_connectors {
        let catalog_names: HashSet<String> = prepared
            .spawn_tool_catalog()
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let selected: HashSet<&str> = requested
            .iter()
            .copied()
            .filter(|name| catalog_names.contains(*name))
            .collect();
        if selected.is_empty() {
            continue;
        }

        tools.extend(
            prepared
                .sub_agent_tools()
                .into_iter()
                .filter(|tool| selected.contains(tool.name().as_str())),
        );
        if let Some(guardrail) = prepared.spawn_guardrails() {
            guardrails.push(guardrail);
        }
    }

    if requested.contains(SearchWebTool::NAME) {
        tools.push(Box::new(SearchWebTool::new(Arc::clone(input.resources))) as Box<dyn ToolDyn>);
    }

    ResolvedSpawnSelection { tools, guardrails }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposeSpawnedPreambleInput, ToolCatalogEntry, ToolCatalogGroup,
        compose_spawned_sub_agent_preamble, render_spawn_tool_catalog, spawn_catalog_tool_names,
    };

    fn sample_groups() -> Vec<ToolCatalogGroup> {
        vec![
            ToolCatalogGroup {
                source: "Datadog".to_string(),
                entries: vec![
                    ToolCatalogEntry::new("search_datadog_logs", "Search Datadog logs."),
                    ToolCatalogEntry::new("get_datadog_metric", "Query a Datadog metric."),
                ],
            },
            ToolCatalogGroup {
                source: "GitHub".to_string(),
                entries: vec![ToolCatalogEntry::new("search_code", "Search code.")],
            },
            ToolCatalogGroup {
                source: "esa".to_string(),
                entries: Vec::new(),
            },
        ]
    }

    #[test]
    fn renders_catalog_grouped_by_source_with_one_line_per_tool() {
        let catalog = render_spawn_tool_catalog(&sample_groups());

        assert_eq!(
            catalog,
            "## Datadog\n- search_datadog_logs: Search Datadog logs.\n- get_datadog_metric: Query a Datadog metric.\n\n## GitHub\n- search_code: Search code."
        );
    }

    #[test]
    fn collects_tool_names_across_groups_preserving_order() {
        let names = spawn_catalog_tool_names(&sample_groups());

        assert_eq!(
            names,
            vec!["search_datadog_logs", "get_datadog_metric", "search_code"]
        );
    }

    #[test]
    fn composes_preamble_with_mission_and_guardrails() {
        let guardrails = vec!["## Mandatory GitHub scope rules\nUse org:acme.".to_string()];
        let preamble = compose_spawned_sub_agent_preamble(&ComposeSpawnedPreambleInput {
            language: "Japanese",
            additional_system_prompt: Some("Prefer runbook links first."),
            lead_instructions: "Investigate the checkout error spike and identify the trigger.",
            guardrails: &guardrails,
        });

        assert!(preamble.contains("Use Japanese for all responses."));
        assert!(preamble.contains(
            "## Mission\nInvestigate the checkout error spike and identify the trigger."
        ));
        assert!(preamble.contains("## Mandatory GitHub scope rules\nUse org:acme."));
        assert!(preamble.contains("## Using Memory Context"));
        assert!(preamble.contains("reili_memory_v1"));
        assert!(preamble.contains(
            "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
        ));
    }

    #[test]
    fn composes_preamble_without_guardrails_section_when_empty() {
        let preamble = compose_spawned_sub_agent_preamble(&ComposeSpawnedPreambleInput {
            language: "English",
            additional_system_prompt: None,
            lead_instructions: "Check external status pages.",
            guardrails: &[],
        });

        assert!(preamble.contains("## Mission\nCheck external status pages.\n\n## Evidence"));
    }
}
