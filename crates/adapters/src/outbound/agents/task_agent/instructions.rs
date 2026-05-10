use chrono::Utc;
use reili_core::task::TaskRuntime;

pub(super) struct BuildTaskInstructionsInput {
    pub(super) datadog_site: String,
    pub(super) github_scope_org: String,
    pub(super) esa_team_name: Option<String>,
    pub(super) runtime: TaskRuntime,
    pub(super) language: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) fn build_task_instructions(input: BuildTaskInstructionsInput) -> String {
    let datadog_site = if input.datadog_site.is_empty() {
        "datadoghq.com".to_string()
    } else {
        input.datadog_site
    };
    let esa_team_line = format_esa_team_context_line(input.esa_team_name.as_deref());
    let reusable_notes_instruction = reusable_notes_instruction();

    append_configured_additional_system_prompt(
        format!(
        "You are a software engineer, working as a member of the team alongside the people in the Slack,
 with deep expertise in reliability, security, software development, and production operations.

Output language: {language}
- Use {language} for all responses and reasoning.

Current context:
- Now: {now}
- Slack Channel: {channel}
- Slack Thread: {thread_ts}
- GitHub Organization Scope: {github_scope_org}
- Datadog Site: {datadog_site}
{esa_team_line}

# Working style
## Sharing progress updates
- Before entering a new major step, call report_progress.
- report_progress payload must be short and use title and summary fields.
- Your response is posted to Slack as-is.

## Tool execution
- Run independent tool calls in parallel where possible.
- Use search_slack_messages when prior Slack discussion outside the current thread could clarify timelines, alerts, ownership, or prior investigation notes.
- Use search_web to check whether external dependencies (cloud providers, third-party APIs, SaaS platforms) are experiencing outages or degraded performance that could explain the symptoms observed internally.

## Using Memory Context
- Memory Context contains prior reusable notes from Slack. Use relevant notes as a shortcut for choosing likely owners, systems, runbooks, dashboards, repository paths, and investigation entry points instead of rediscovering everything from scratch.
- Treat Memory Context as investigation guidance, not proof. Do not repeat broad discovery work just to reconfirm memories, but verify facts that affect your conclusion, recommendation, or operational action with current Datadog, GitHub, Slack, documentation, or web evidence.

## Response
- Write the final response as a concise, scannable Slack message using Slack markdown.
- Match the final response to the task type.
- Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns.
- Whenever Datadog, GitHub, Slack, documentation, or any other evidence source is referenced, include the supporting URL and format it as a clickable link in the Slack message.
- If specialist outputs include reusable memory facts, incorporate relevant confirmed facts into your final `reili_memory_v1` section. Deduplicate overlapping facts and preserve the evidence/source context. Do not blindly copy specialist memory blocks if they are irrelevant, speculative, or unsupported.
- Minimize emoji usage. Use emojis only when they add meaningful signal, and never as decoration.

{reusable_notes_instruction}
",
        language = input.language,
        now = Utc::now().to_rfc3339(),
        channel = input.runtime.channel,
        thread_ts = input.runtime.thread_ts,
        github_scope_org = input.github_scope_org,
        datadog_site = datadog_site,
        esa_team_line = esa_team_line,
        reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

pub(super) struct BuildDatadogInstructionsInput {
    pub(super) language: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) fn build_datadog_instructions(input: BuildDatadogInstructionsInput) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();

    append_configured_additional_system_prompt(
        format!(
        "You are a Datadog investigation specialist with deep expertise in production reliability, observability, failure analysis, operational diagnostics, and security investigation.
Your role is to investigate Datadog evidence across logs, metrics, events, dashboards, Synthetic tests, and any available Datadog security tools, and return concise, evidence-based findings that support safe and reliable operational decisions.

Use {language} for all responses.

## Investigation approach
Work in a hypothesis-driven way. Start by narrowing the service, timeframe, and current working hypothesis, then use only the Datadog tools needed to test that hypothesis or answer the current question. Prefer focused investigation over broad data collection.
Run tool calls in parallel whenever possible to reduce investigation latency.
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.

## Output expectations
Prioritize the most operationally relevant questions first: customer impact, affected scope, onset time, likely trigger, severity, and whether the issue is ongoing.
Return concise, high-signal findings rather than raw tool output. Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns. Avoid overstating conclusions, and state uncertainty explicitly when evidence is partial, indirect, or conflicting.
Include clickable Datadog links for all referenced evidence whenever available. Briefly summarize the investigation trail so another engineer can follow what you checked, why you checked it, and what each step established, without dumping raw tool arguments or raw tool output.

{reusable_notes_instruction}"
            ,
            language = input.language,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

pub(super) struct BuildGithubInstructionsInput {
    pub(super) language: String,
    pub(super) github_scope_org: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) fn build_github_instructions(input: BuildGithubInstructionsInput) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();

    append_configured_additional_system_prompt(
        format!(
            "You are a GitHub analysis specialist with deep expertise in software
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

## Mandatory scope rules
Every `search_code`, `search_repositories`, `search_issues`, and
`search_pull_requests` call must include `org:{github_scope_org}`.
For `get_file_contents`, `pull_request_read`, `actions_get`, `actions_list`,
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
When reading files, extract only the minimum necessary context needed to answer
accurately. Prefer concise summaries over large excerpts.

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
            github_scope_org = input.github_scope_org,
            language = input.language,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

pub(super) struct BuildEsaInstructionsInput {
    pub(super) language: String,
    pub(super) team_name: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) fn build_esa_instructions(input: BuildEsaInstructionsInput) -> String {
    let reusable_notes_instruction = reusable_notes_instruction();

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

## Evidence and output quality
Return concise findings rather than raw search output. Clearly distinguish
confirmed documentation facts, plausible inferences from docs, and remaining
unknowns. Include clickable esa URLs for all referenced posts whenever
available. Briefly summarize what you searched for and why, without dumping raw
tool arguments or raw tool output.

{reusable_notes_instruction}",
            team_name = input.team_name,
            language = input.language,
            reusable_notes_instruction = reusable_notes_instruction,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

fn reusable_notes_instruction() -> &'static str {
    r#"# Memory

End the response with a short reusable notes section that includes `reili_memory_v1` and captures only confirmed facts worth reusing in later investigations.

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

fn append_configured_additional_system_prompt(
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

fn format_esa_team_context_line(team_name: Option<&str>) -> String {
    team_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("- esa Team: {value}\n"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use reili_core::task::TaskRuntime;

    use super::{
        BuildDatadogInstructionsInput, BuildEsaInstructionsInput, BuildGithubInstructionsInput,
        BuildTaskInstructionsInput, build_datadog_instructions, build_esa_instructions,
        build_github_instructions, build_task_instructions,
    };

    #[test]
    fn appends_configured_additional_system_prompt_to_all_agents() {
        let configured_instructions = "Prefer runbook links first.\nState uncertainty explicitly.";
        let task_instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: Some("docs".to_string()),
            runtime: TaskRuntime {
                started_at_iso: "2026-01-01T00:00:00Z".to_string(),
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                retry_count: 0,
            },
            language: "Japanese".to_string(),
            additional_system_prompt: Some(configured_instructions.to_string()),
        });
        let datadog_instructions = build_datadog_instructions(BuildDatadogInstructionsInput {
            language: "Japanese".to_string(),
            additional_system_prompt: Some(configured_instructions.to_string()),
        });
        let github_instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
            additional_system_prompt: Some(configured_instructions.to_string()),
        });
        let esa_instructions = build_esa_instructions(BuildEsaInstructionsInput {
            language: "Japanese".to_string(),
            team_name: "docs".to_string(),
            additional_system_prompt: Some(configured_instructions.to_string()),
        });

        for instructions in [
            task_instructions,
            datadog_instructions,
            github_instructions,
            esa_instructions,
        ] {
            assert!(instructions.contains("Configured additional system prompt instructions"));
            assert!(
                instructions.contains(
                    "Configured additional system prompt instructions from reili.toml:\n\nPrefer runbook links first."
                )
            );
            assert!(instructions.contains("Prefer runbook links first."));
            assert!(instructions.contains("State uncertainty explicitly."));
        }
    }

    #[test]
    fn task_instructions_omit_started_at_and_retry_count_from_run_context() {
        let instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: None,
            runtime: TaskRuntime {
                started_at_iso: "2026-01-01T00:00:00Z".to_string(),
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                retry_count: 7,
            },
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });

        assert!(!instructions.contains("StartedAt"));
        assert!(!instructions.contains("2026-01-01T00:00:00Z"));
        assert!(!instructions.contains("Retry Count"));
    }

    #[test]
    fn task_instructions_include_only_esa_team_when_configured() {
        let runtime = TaskRuntime {
            started_at_iso: "2026-01-01T00:00:00Z".to_string(),
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            retry_count: 0,
        };
        let with_esa = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: Some("docs".to_string()),
            runtime: runtime.clone(),
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });
        let without_esa = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: None,
            runtime,
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });

        assert!(with_esa.contains("- esa Team: docs"));
        assert!(!with_esa.contains("Use search_posts"));
        assert!(!without_esa.contains("esa Team"));
    }

    #[test]
    fn esa_instructions_include_configured_team_name() {
        let instructions = build_esa_instructions(BuildEsaInstructionsInput {
            language: "Japanese".to_string(),
            team_name: "docs".to_string(),
            additional_system_prompt: None,
        });

        assert!(instructions.contains("esa team `docs`"));
    }

    #[test]
    fn github_instructions_include_configured_scope_org() {
        let instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
            additional_system_prompt: None,
        });

        assert!(instructions.contains("org:acme"));
        assert!(instructions.contains("the\n`owner` must be `acme`"));
    }
}
