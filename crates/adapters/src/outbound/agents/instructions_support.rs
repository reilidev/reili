//! Shared preamble building blocks used by the lead task instructions and by each connector's
//! sub-agent preamble. Keeping these here lets connector modules (`mcp/<svc>`, `connectors/<svc>`)
//! and `task_agent` reuse the same memory/notes guidance without depending on each other.

pub(crate) fn sub_agent_memory_context_instruction() -> &'static str {
    r#"## Using Memory Context
If the delegated task prompt includes Memory Context, use relevant memories as investigation guidance for likely owners, systems, runbooks, dashboards, repository paths, and search terms. Treat memories as hints, not proof. Verify facts that affect your conclusion, recommendation, or operational action with current evidence from your available tools. Do not copy, paraphrase, or refresh prior memory entries into reusable notes. Only save a memory when the fact was newly learned or independently confirmed during this investigation, and cite current non-memory evidence."#
}

pub(crate) fn reusable_notes_instruction() -> &'static str {
    r#"# Memory

End the response with a short reusable notes section that includes `reili_memory_v1` only when there are new or independently confirmed facts worth reusing in later investigations. If there are no such facts, omit the `reili_memory_v1` marker entirely.

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

When saving memories, include `reili_memory_v1` once and use this structure:
reili_memory_v1
- Fact: A concise, specific statement of the verified fact.
- Evidence: Where this was confirmed, such as file path, document name, ticket, runbook, log source, monitoring dashboard, or user instruction.
- Scope: The project, repository, service, environment, workflow, or operational area where this applies.

When saving multiple memories, separate each fact block with `---`. Do not use `Memory:` labels.

Example:

reili_memory_v1
- Fact: Production feature flag changes require approval from the on-call engineer before rollout.
- Evidence: Confirmed in the release runbook.
- Scope: Production release and feature rollout workflow.
---
- Fact: API latency investigations should start from the service latency dashboard.
- Evidence: Confirmed in the incident response runbook.
- Scope: API production incident triage.

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
