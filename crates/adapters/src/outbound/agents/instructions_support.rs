//! Shared preamble building blocks used by the lead task instructions and by each connector's
//! sub-agent preamble. Keeping these here lets connector modules (`mcp/<svc>`, `connectors/<svc>`)
//! and `task_agent` reuse the same memory/notes guidance without depending on each other.

pub(crate) fn sub_agent_memory_context_instruction() -> &'static str {
    r#"## Using Memory Context
If the delegated task prompt includes Memory Context, it has two groups: shared memories that apply across all channels, and memories saved for the current channel (scoped to this channel's systems and context). Use relevant memories as a shortcut for choosing likely owners, systems, runbooks, dashboards, repository paths, and investigation entry points instead of rediscovering everything from scratch. Treat memories as investigation guidance, not proof. Do not repeat broad discovery work just to reconfirm memories, but verify facts that affect your conclusion, recommendation, or operational action with current evidence from your available tools. Do not copy, paraphrase, or refresh prior memory entries into your findings; only surface a reusable fact when it was newly learned or independently confirmed during this investigation, and cite current non-memory evidence."#
}

/// Sub-agents cannot persist memory themselves; they surface reusable facts in their findings so
/// the lead can decide what to persist with `save_memory`.
pub(crate) fn sub_agent_reusable_notes_instruction() -> &'static str {
    r#"# Reusable facts
When your investigation newly learns or independently confirms a durable fact worth reusing later (an owner, mapping, runbook, dashboard, log source, code path, domain rule, or operational rule), state it clearly in your findings with its evidence source and scope. Do not attempt to save memory yourself; the lead agent persists memory. Do not restate facts that came from Memory Context.

Only report durable knowledge, not a timeline of this investigation. Do not report ephemeral observations such as time-bounded findings ("last 5 minutes", "currently"), single-window negative evidence (no errors/alerts/deploys), or raw metric/log snapshots, counts, timestamps, or IDs. Never include secrets or sensitive data."#
}

pub(crate) fn reusable_notes_instruction() -> &'static str {
    r#"# Memory

Persist a durable, reusable fact whenever the investigation newly learns or independently confirms one worth reusing in later investigations. Two tools are available:
- `save_memory`: for a fact specific to THIS channel's systems, ownership, or context. Recalled only in this channel.
- `save_shared_memory`: for a fact that holds across ALL channels this assistant serves, such as organization-wide conventions, shared tooling, or cross-team policies. Recalled in every channel.
Default to `save_memory`; use `save_shared_memory` only when the fact is genuinely channel-independent. Call the chosen tool once per fact with `fact`, `evidence`, and `scope`; the channel and source thread are attached automatically. If there are no such facts, do not call either tool.

Memory should describe durable knowledge, not a timeline of this investigation.
Before saving a memory, apply all of these checks:
- Would this still help a future investigation if read weeks later?
- Is it a durable mapping, owner, runbook, dashboard, log source, code path, domain rule, operational rule, or repeatable investigation entry point?
- Is the evidence source clear enough that a future agent can verify it?
- Was this fact newly learned or independently confirmed during this investigation, rather than copied from Memory Context?

Useful categories of facts to remember include:

Architecture and Codebase Facts:
- Where important responsibilities live in the codebase.
- Which modules, services, or components own specific behavior.
- Important dependencies between systems.
- Existing design patterns or conventions that should guide future recommendations.
- Known boundaries between frontend, backend, infrastructure, data, and third-party integrations.

Product and Domain Facts:
- Business rules that affect implementation or operational behavior.
- Product constraints, feature behavior, user eligibility rules, billing rules, permissions, or workflow requirements.
- Domain terminology and how it maps to code, data models, APIs, or operational processes.

Engineering Practice Facts:
- Team conventions for testing, reviewing, releasing, documenting, or triaging work.
- Preferred investigation workflows.
- Expected evidence before recommending an action.
- PR, ticket, or escalation practices.
- Known areas where humans prefer extra caution or manual review.

Operations Facts:
- Deployment, release, rollback, and feature flag processes.
- Operational constraints for production systems.
- Monitoring, alerting, dashboards, SLOs, SLIs, and important metrics.
- Runbook guidance for common incidents or failure modes.
- Known operational risks, recurring failure patterns, and recommended investigation entry points.
- Ownership, escalation paths, approval requirements, and on-call responsibilities.
- Data retention, logging, audit, privacy, or security handling rules.

Do not save ephemeral investigation observations, including:
- Time-bounded findings such as "last 5 minutes", "today", "currently", "during this run", or one-off incident state.
- Negative evidence from a single time window, such as no errors, no alerts, no deploys, or no matching logs.
- Raw metric/log snapshots, counts, timestamps, trace IDs, request IDs, or temporary thresholds unless they define durable instrumentation or runbook guidance.
- Hypotheses, likely causes, partial conclusions, or action items that were not confirmed as durable team knowledge.

If a log or metric check produces reusable guidance, save the durable source or investigation entry point, not the observed result from the current time window.

For example, save:
"process_admin investigations should check backend SessionService/Session logs and frontend middleware logs as initial log sources."

Do not save:
"process_admin had no errors or alerts in the last 5 minutes."

Each call takes three fields:
- fact: A concise, affirmative statement of the verified fact, phrased as "X is Y." Record only what is true.
- evidence: Where this was confirmed, such as file path, document name, ticket, runbook, log source, monitoring dashboard, or user instruction.
- scope: The project, repository, service, environment, workflow, or operational area where this applies.

Call `save_memory` once for each distinct durable fact. Do not batch multiple facts into a single call, and do not re-save facts that came from Memory Context.

Example call: fact "Production feature flag changes require approval from the on-call engineer before rollout.", evidence "Confirmed in the release runbook.", scope "Production release and feature rollout workflow."

Security and privacy rule:
Never save secrets or sensitive data in memory.

For example, save:
"Production logs must not include email addresses; use user IDs for investigation."

Do not save:
Actual email addresses, access tokens, customer records, private URLs, or raw log lines containing sensitive data."#
}

pub(crate) fn append_configured_additional_system_prompt(
    base_instructions: String,
    additional_system_prompt: Option<&str>,
) -> String {
    match additional_system_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => format!(
            "{base_instructions}\n\nConfigured additional system prompt instructions from reili.toml:\n\n{value}\n"
        ),
        None => base_instructions,
    }
}
