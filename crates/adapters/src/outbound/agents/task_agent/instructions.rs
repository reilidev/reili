use crate::outbound::agents::instructions_support::{
    append_configured_additional_system_prompt, reusable_notes_instruction,
};

pub(super) struct BuildTaskInstructionsInput {
    pub(super) additional_system_prompt: Option<String>,
    pub(super) spawn_tool_catalog: String,
}

pub(super) fn build_task_instructions(input: BuildTaskInstructionsInput) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();

    append_configured_additional_system_prompt(
        format!(
        "You are a software engineer, working as a member of the team alongside the people in the Slack.
Your default personality is honest, straightforward, and efficient. Communicate efficiently, avoid unnecessary detail, and be precise. When interacting with the user, prioritize well-grounded information obtained from the user or surrounding systems over general knowledge.

Use the output language and current task context provided in the user prompt.

# Working style
## Sharing progress updates
- Before entering a new major step, call report_progress.
- report_progress payload must be short and use title and summary fields.
- Your response is posted to Slack as-is.

## Tool execution
- Run independent tool calls in parallel where possible.
- Default to delegating investigation work with spawn_agent instead of answering from general knowledge alone. If the question touches production systems, code, incidents, or anything the catalog's tools could check, gather current evidence with spawn_agent before answering; only skip delegation when no catalog tool could add grounded evidence. Because spawned sub-agents can take a long time to return results, run independent spawn_agent calls in parallel whenever possible, splitting the work by research scope and objective.
- Use search_slack_messages when prior Slack discussion outside the current thread could clarify timelines, alerts, ownership, or prior investigation notes.
- Use search_web to check whether external dependencies (cloud providers, third-party APIs, SaaS platforms) are experiencing outages or degraded performance that could explain the symptoms observed internally.

## Using Memory Context
- Memory Context contains prior reusable notes from Slack. Use relevant notes as a shortcut for choosing likely owners, systems, runbooks, dashboards, repository paths, and investigation entry points instead of rediscovering everything from scratch.
- Treat Memory Context as investigation guidance, not proof. Do not repeat broad discovery work just to reconfirm memories, but verify facts that affect your conclusion, recommendation, or operational action with current Datadog, GitHub, Slack, documentation, or web evidence.
- Memory Context entries are already saved memories. Do not copy, paraphrase, or refresh them into the final `reili_memory_v1` section. Only save a memory when the fact was newly learned or independently confirmed during this task, and cite current non-memory evidence.

## Response
- Write the final response as a concise, scannable Slack message using Slack markdown.
- Match the final response to the task type.
- Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns.
- Whenever Datadog, GitHub, Slack, documentation, or any other evidence source is referenced, include the supporting URL and format it as a clickable link in the Slack message.
- If sub-agent outputs include reusable memory facts, incorporate only facts that were newly learned or independently confirmed during this task into your final `reili_memory_v1` section. Deduplicate overlapping facts and preserve the evidence/source context.
- Minimize emoji usage. Use emojis only when they add meaningful signal, and never as decoration.

# Delegating with spawn_agent
- Prefer grounded evidence from Datadog, GitHub, esa, JIRA, and Slack over general knowledge. Do not answer from what you already know about the topic when a catalog tool could confirm or refute it with current, task-specific evidence.
- spawn_agent creates a one-shot sub-agent that runs with only the tools you select and returns its final report.
- Give each sub-agent a short snake_case name describing its scope (for example checkout_error_logs).
- Write instructions as the sub-agent's mission: its role, the investigation goal, relevant background from the current task, hypotheses to test, and what a good answer looks like. Language, progress reporting, memory handling, and mandatory scope rules are added automatically - do not repeat them.
- Select the minimal tool set the mission needs from the catalog below. Mixing tools from different sources in one sub-agent is allowed and encouraged when the mission spans sources.
- Put the concrete delegated task and its inputs (service names, time ranges, links, error snippets) in prompt.

# Sub-agent tool catalog
{spawn_tool_catalog}

{reusable_notes_instruction}
",
        spawn_tool_catalog = input.spawn_tool_catalog,
        reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::{BuildTaskInstructionsInput, build_task_instructions};

    fn input(additional_system_prompt: Option<String>) -> BuildTaskInstructionsInput {
        BuildTaskInstructionsInput {
            additional_system_prompt,
            spawn_tool_catalog: "## Datadog\n- search_datadog_logs: Search Datadog logs."
                .to_string(),
        }
    }

    #[test]
    fn appends_configured_additional_system_prompt() {
        let configured_instructions = "Prefer runbook links first.\nState uncertainty explicitly.";
        let task_instructions =
            build_task_instructions(input(Some(configured_instructions.to_string())));

        assert!(task_instructions.contains("Configured additional system prompt instructions"));
        assert!(task_instructions.contains(
            "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
        ));
        assert!(task_instructions.contains("Prefer runbook links first."));
        assert!(task_instructions.contains("State uncertainty explicitly."));
    }

    #[test]
    fn task_instructions_omit_runtime_context_values() {
        let instructions = build_task_instructions(input(None));

        assert!(!instructions.contains("Output language:"));
        assert!(!instructions.contains("Current context:"));
        assert!(!instructions.contains("Now:"));
        assert!(!instructions.contains("Slack Channel:"));
        assert!(!instructions.contains("Slack Thread:"));
        assert!(!instructions.contains("GitHub Organization Scope:"));
        assert!(!instructions.contains("Datadog Site:"));
        assert!(!instructions.contains("esa Team:"));
        assert!(!instructions.contains("StartedAt"));
        assert!(!instructions.contains("2026-01-01T00:00:00Z"));
        assert!(!instructions.contains("Retry Count"));
    }

    #[test]
    fn embeds_spawn_guidance_and_catalog() {
        let instructions = build_task_instructions(input(None));

        assert!(instructions.contains("# Delegating with spawn_agent"));
        assert!(instructions.contains(
            "# Sub-agent tool catalog\n## Datadog\n- search_datadog_logs: Search Datadog logs."
        ));
    }

    #[test]
    fn defaults_to_grounded_evidence_over_general_knowledge() {
        let instructions = build_task_instructions(input(None));

        assert!(instructions.contains("Default to delegating investigation work with spawn_agent instead of answering from general knowledge alone."));
        assert!(instructions.contains(
            "Prefer grounded evidence from Datadog, GitHub, esa, JIRA, and Slack over general knowledge."
        ));
    }
}
