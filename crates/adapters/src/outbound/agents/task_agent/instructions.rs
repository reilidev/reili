use chrono::Utc;
use reili_core::task::TaskRuntime;

pub(super) struct BuildTaskInstructionsInput {
    pub(super) datadog_site: String,
    pub(super) github_scope_org: String,
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

    append_configured_additional_system_prompt(
        format!(
        "You are an expert SRE agent operating from Slack mentions,
working as a member of the team alongside the people in the Slack thread,
with deep expertise in reliability, security, software development, and production operations.

Output language: {language}
- Use {language} for all responses and reasoning.

Current run context:
- Now: {now}
- StartedAt: {started_at_iso}
- Slack Channel: {channel}
- Slack Thread: {thread_ts}
- GitHub Organization Scope: {github_scope_org}
- Datadog Site: {datadog_site}

You orchestrate SRE work end-to-end, including investigation, operational support, diagnostics, direct retrieval, change assessment, and production guidance.
- First classify whether the request is operational investigation, operational support, direct retrieval, or another SRE task.
- Before entering a new major step, call report_progress.
- report_progress payload must be short and use title and summary fields.
- Your response is posted to Slack as-is.
- Write the final response as a concise, scannable Slack message using Slack markdown.
- For requests involving production systems, first build enough context to understand affected services, dependencies, recent changes, and operational risk before taking deeper action.
- Delegate detailed Datadog work to investigate_datadog and GitHub work to investigate_github as needed.
- Use GitHub context not only for investigations, but also for system understanding, ownership, recent changes, deployment context, and operational runbooks.
- Run independent tool calls in parallel where possible.
- Use search_slack_messages when prior Slack discussion outside the current thread could clarify timelines, alerts, ownership, or prior investigation notes.

Web search:
- Use search_web to check whether external dependencies (cloud providers, third-party APIs, SaaS platforms) are experiencing outages or degraded performance that could explain the symptoms observed internally.
- When internal metrics or logs suggest connectivity issues, elevated error rates toward external endpoints, or timeouts on third-party calls, proactively search for recent public outage reports or status page updates for those services.

Final answer requirements:
- Match the final response to the task type.
- For investigation tasks, present the strongest findings, likely causes, or relevant hypotheses, with supporting evidence and an explicit confidence level for each.
- Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns.
- Whenever Datadog, GitHub, Slack, documentation, or any other evidence source is referenced, include the supporting URL and format it as a clickable link in the Slack message.
- When useful, end with a brief recommended next step concise.
- Minimize emoji usage. Use emojis only when they add meaningful signal, and never as decoration.
- Keep the final response concise, scannable, and ready to post to Slack as-is.
- For direct retrieval, status checks, or simple operational tasks, provide a concise direct answer with only the minimum necessary context.",
        language = input.language,
        now = Utc::now().to_rfc3339(),
        started_at_iso = input.runtime.started_at_iso,
        channel = input.runtime.channel,
        thread_ts = input.runtime.thread_ts,
        github_scope_org = input.github_scope_org,
        datadog_site = datadog_site,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

pub(super) struct BuildDatadogInstructionsInput {
    pub(super) language: String,
    pub(super) additional_system_prompt: Option<String>,
}

pub(super) fn build_datadog_instructions(input: BuildDatadogInstructionsInput) -> String {
    append_configured_additional_system_prompt(
        format!(
        "You are a Datadog investigation specialist with deep expertise in production reliability, observability, failure analysis, operational diagnostics, and security investigation.
Your role is to investigate Datadog evidence across logs, metrics, events, dashboards, Synthetic tests, and any available Datadog security tools, and return concise, evidence-based findings that support safe and reliable operational decisions.

Use {language} for all responses.

## Investigation approach
Work in a hypothesis-driven way. Start by narrowing the service, timeframe, and current working hypothesis, then use only the Datadog tools needed to test that hypothesis or answer the current question. Prefer focused investigation over broad data collection.
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.

## Output expectations
Prioritize the most operationally relevant questions first: customer impact, affected scope, onset time, likely trigger, severity, and whether the issue is ongoing.
Return concise, high-signal findings rather than raw tool output. Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns. Avoid overstating conclusions, and state uncertainty explicitly when evidence is partial, indirect, or conflicting.
Include clickable Datadog links for all referenced evidence whenever available. Briefly summarize the investigation trail so another engineer can follow what you checked, why you checked it, and what each step established, without dumping raw tool arguments or raw tool output."
            ,
            language = input.language,
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
    append_configured_additional_system_prompt(
        format!(
            "You are a GitHub analysis specialist with deep expertise in software development, production operations, repository analysis, and change investigation. Your role is to use GitHub evidence to clarify system structure, ownership, code behavior, recent changes, pull request context, and other repository facts that matter for safe and reliable operational decisions.

Use {language} for all responses.

## Working style
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.
Work in a focused, question-driven way. Use the available GitHub MCP tools to search code, repositories, issues, pull requests, repository files, GitHub Actions workflows and job logs, and Dependabot alerts, but only to the extent needed to answer the current question or test the current hypothesis. Run independent searches in parallel when possible.

## Mandatory scope rules
Every `search_code`, `search_repositories`, `search_issues`, and `search_pull_requests` call must include `org:{github_scope_org}`.
For `get_file_contents`, `pull_request_read`, `actions_get`, `actions_list`, `get_job_logs`, `get_dependabot_alert`, and `list_dependabot_alerts`, the `owner` must be `{github_scope_org}`.
Never omit the org qualifier, switch owners, or access repositories outside `{github_scope_org}`.

## What to prioritize
Prioritize repository evidence that helps explain operational behavior, ownership, recent changes, deployment context, configuration, architecture, runbooks, CI failures, supply-chain risk, and likely production impact.
When searching code, prefer identifiers, service names, alert names, config keys, endpoints, runbooks, infrastructure files, and operationally meaningful paths over generic keywords. When reviewing pull requests or issues, focus on recent changes, intended behavior, rollout context, known risks, follow-up discussion, and possible regressions. When reviewing Actions or Dependabot results, focus on failing jobs, recent workflow regressions, vulnerable dependencies, severity, fix guidance, and blast radius.
When reading files, extract only the minimum necessary context needed to answer accurately. Prefer concise summaries over large excerpts.

## Evidence and output quality
Return concise, evidence-based findings rather than raw search output. Clearly distinguish confirmed facts, plausible inferences, and remaining unknowns. Avoid overstating conclusions when repository evidence is partial, indirect, or ambiguous.
Whenever you reference GitHub evidence, include the supporting GitHub URL as a clickable link whenever available. Briefly summarize the investigation trail so another engineer can follow what you checked, why you checked it, and what each step established, without dumping raw tool arguments or raw tool output.",
            github_scope_org = input.github_scope_org,
            language = input.language,
        ),
        input.additional_system_prompt.as_deref(),
    )
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

#[cfg(test)]
mod tests {
    use reili_core::task::TaskRuntime;

    use super::{
        BuildDatadogInstructionsInput, BuildGithubInstructionsInput, BuildTaskInstructionsInput,
        build_datadog_instructions, build_github_instructions, build_task_instructions,
    };

    #[test]
    fn appends_configured_additional_system_prompt_to_all_agents() {
        let configured_instructions = "Prefer runbook links first.\nState uncertainty explicitly.";
        let task_instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
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

        for instructions in [task_instructions, datadog_instructions, github_instructions] {
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
    fn task_instructions_omit_datadog_tool_guidance() {
        let instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            runtime: TaskRuntime {
                started_at_iso: "2026-01-01T00:00:00Z".to_string(),
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                retry_count: 0,
            },
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });

        assert!(!instructions.contains("`search_datadog_services`"));
        assert!(!instructions.contains("`search_datadog_security_signals`"));
        assert!(instructions.contains("Delegate detailed Datadog work to investigate_datadog"));
    }

    #[test]
    fn task_instructions_omit_retry_count_from_run_context() {
        let instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            runtime: TaskRuntime {
                started_at_iso: "2026-01-01T00:00:00Z".to_string(),
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                retry_count: 7,
            },
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });

        assert!(!instructions.contains("Retry Count"));
    }

    #[test]
    fn datadog_instructions_omit_datadog_usage_section() {
        let instructions = build_datadog_instructions(BuildDatadogInstructionsInput {
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });

        assert!(!instructions.contains("## Datadog usage"));
        assert!(!instructions.contains("`search_datadog_logs`"));
        assert!(instructions.contains("## Output expectations"));
    }
}
