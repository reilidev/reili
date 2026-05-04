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

    append_configured_additional_system_prompt(
        format!(
        "You are a software engineer, working as a member of the team alongside the people in the Slack,
 with deep expertise in reliability, security, software development, and production operations.

Output language: {language}
- Use {language} for all responses and reasoning.

Current context:
- Now: {now}
- StartedAt: {started_at_iso}
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

## Response
- Write the final response as a concise, scannable Slack message using Slack markdown.
- Match the final response to the task type.
- Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns.
- Whenever Datadog, GitHub, Slack, documentation, or any other evidence source is referenced, include the supporting URL and format it as a clickable link in the Slack message.
- End the response with a short reusable notes section that captures discovered facts and repository structure details worth reusing in later investigations.
- Minimize emoji usage. Use emojis only when they add meaningful signal, and never as decoration.
",
        language = input.language,
        now = Utc::now().to_rfc3339(),
        started_at_iso = input.runtime.started_at_iso,
        channel = input.runtime.channel,
        thread_ts = input.runtime.thread_ts,
        github_scope_org = input.github_scope_org,
        datadog_site = datadog_site,
        esa_team_line = esa_team_line,
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
needed to answer the current question or test the current hypothesis. Run
independent searches in parallel when possible.

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
At the end of your response, add a short reusable notes section that captures
discovered facts and repository structure details worth reusing in later
investigations.",
            github_scope_org = input.github_scope_org,
            language = input.language,
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
Do not narrow your search to operational or investigation terms when the
request is asking for broader internal knowledge.

## Evidence and output quality
Return concise findings rather than raw search output. Clearly distinguish
confirmed documentation facts, plausible inferences from docs, and remaining
unknowns. Include clickable esa URLs for all referenced posts whenever
available. Briefly summarize what you searched for and why, without dumping raw
tool arguments or raw tool output.",
            team_name = input.team_name,
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
    fn task_instructions_omit_datadog_tool_guidance() {
        let instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: None,
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

        assert!(!instructions.contains("Retry Count"));
    }

    #[test]
    fn task_instructions_include_reusable_notes_guidance() {
        let instructions = build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: "datadoghq.com".to_string(),
            github_scope_org: "acme".to_string(),
            esa_team_name: None,
            runtime: TaskRuntime {
                started_at_iso: "2026-01-01T00:00:00Z".to_string(),
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                retry_count: 0,
            },
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });
        let normalized_instructions = instructions
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(normalized_instructions.contains(
            "End the response with a short reusable notes section that captures discovered facts and repository structure details worth reusing in later investigations."
        ));
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
    fn esa_instructions_include_general_internal_knowledge_scope() {
        let instructions = build_esa_instructions(BuildEsaInstructionsInput {
            language: "Japanese".to_string(),
            team_name: "docs".to_string(),
            additional_system_prompt: None,
        });
        let normalized_instructions = instructions
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(instructions.contains("esa team `docs`"));
        assert!(instructions.contains("Use search_posts"));
        assert!(instructions.contains("team processes"));
        assert!(instructions.contains("product specifications"));
        assert!(normalized_instructions.contains(
            "whether the topic is operational, architectural, procedural, or organizational"
        ));
        assert!(
            normalized_instructions
                .contains("Do not narrow your search to operational or investigation terms")
        );
    }

    #[test]
    fn github_instructions_prioritize_readme_and_design_docs_for_initial_exploration() {
        let instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
            additional_system_prompt: None,
        });
        let normalized_instructions = instructions
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(normalized_instructions.contains(
            "When starting exploration for an unfamiliar repository, first read high-signal orientation docs such as README, architecture/design documents, and key technical documentation"
        ));
        assert!(normalized_instructions.contains("before broad search"));
        assert!(normalized_instructions.contains("avoid scattered exploration"));
        assert!(
            normalized_instructions
                .contains("At the end of your response, add a short reusable notes section")
        );
    }
}
