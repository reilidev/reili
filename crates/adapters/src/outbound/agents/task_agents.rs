use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use reili_core::logger::Logger;
use reili_core::task::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskCancellation, TaskProgressEventPort, TaskRequest,
    TaskResources, TaskRuntime,
};
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::agent_execution_hook::AgentExecutionHook;
use super::datadog_mcp_tools::DatadogMcpToolset;
use super::github_mcp_tools::GitHubMcpToolset;
use super::tools::{
    ReportProgressTool, ReportProgressToolInput, SearchSlackMessagesTool, SearchWebTool,
};
use super::{
    llm_provider_settings::LlmProviderSettings, llm_usage_collector::LlmUsageCollector,
    progress_reporting_sub_agent_tool::ProgressReportingSubAgentTool,
};

const DATADOG_PROGRESS_OWNER_ID: &str = "investigate_datadog";
const GITHUB_PROGRESS_OWNER_ID: &str = "investigate_github";

type CompletionAgent<C> = Agent<<C as CompletionClient>::CompletionModel>;
type SpecialistAgent<C> = Agent<<C as CompletionClient>::CompletionModel, AgentExecutionHook>;

pub struct BuildTaskAgentInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub resources: Arc<TaskResources>,
    pub datadog_site: String,
    pub datadog_mcp_toolset: DatadogMcpToolset,
    pub github_mcp_toolset: GitHubMcpToolset,
    pub github_scope_org: String,
    pub logger: Arc<dyn Logger>,
    pub runtime: TaskRuntime,
    pub cancellation: TaskCancellation,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub language: String,
    pub additional_system_prompt: Option<String>,
    pub usage_collector: LlmUsageCollector,
    pub slack_action_token: Option<String>,
}

#[must_use]
pub fn build_task_agent<C>(input: BuildTaskAgentInput<C>) -> CompletionAgent<C>
where
    C: CompletionClient + Clone,
    C::CompletionModel: 'static,
{
    let datadog_agent = build_datadog_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        settings: input.settings.clone(),
        resources: Arc::clone(&input.resources),
        datadog_mcp_toolset: input.datadog_mcp_toolset.clone(),
        github_mcp_toolset: input.github_mcp_toolset.clone(),
        github_scope_org: String::new(),
        language: input.language.clone(),
        additional_system_prompt: input.additional_system_prompt.clone(),
        logger: Arc::clone(&input.logger),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: DATADOG_PROGRESS_OWNER_ID.to_string(),
        runtime: input.runtime.clone(),
        cancellation: input.cancellation.clone(),
        usage_collector: input.usage_collector.clone(),
    });
    let github_agent = build_github_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        settings: input.settings.clone(),
        resources: Arc::clone(&input.resources),
        datadog_mcp_toolset: input.datadog_mcp_toolset.clone(),
        github_mcp_toolset: input.github_mcp_toolset.clone(),
        github_scope_org: input.github_scope_org.clone(),
        language: input.language.clone(),
        additional_system_prompt: input.additional_system_prompt.clone(),
        logger: Arc::clone(&input.logger),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: GITHUB_PROGRESS_OWNER_ID.to_string(),
        runtime: input.runtime.clone(),
        cancellation: input.cancellation.clone(),
        usage_collector: input.usage_collector.clone(),
    });

    let builder = input
        .client
        .agent(input.settings.task_runner_model.clone())
        .name("TaskRunner")
        .preamble(&build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: input.datadog_site,
            github_scope_org: input.github_scope_org,
            runtime: input.runtime,
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }))
        .default_max_turns(input.settings.task_runner_max_turns)
        .additional_params(input.settings.additional_params.clone());
    let builder = with_max_tokens(builder, input.settings.max_tokens);

    builder
        .tools(input.datadog_mcp_toolset.lead_tools())
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: TASK_RUNNER_PROGRESS_OWNER_ID.to_string(),
        }))
        .tool(ProgressReportingSubAgentTool::new(
            datadog_agent,
            DATADOG_PROGRESS_OWNER_ID.to_string(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(SearchSlackMessagesTool::new(
            Arc::clone(&input.resources.slack_message_search_port),
            input.slack_action_token.clone(),
        ))
        .tool(SearchWebTool::new(Arc::clone(&input.resources)))
        .tool(ProgressReportingSubAgentTool::new(
            github_agent,
            GITHUB_PROGRESS_OWNER_ID.to_string(),
            Arc::clone(&input.on_progress_event),
        ))
        .build()
}

#[must_use]
pub fn build_task_prompt(request: &TaskRequest) -> String {
    let task_prompt = "Investigate the following user input and respond with the most appropriate investigation or direct answer.
The input may be an alert, request, question, link, or partial context.";
    let trigger_message_text = request.trigger_message.rendered_text();
    let trigger_message_text = trigger_message_text.trim();
    let trigger_message_section = format!("\n\nTrigger Message: {trigger_message_text}");
    let bot_user_id = extract_mentioned_user_id(&request.trigger_message.text);
    let thread_transcript =
        build_thread_transcript(&request.thread_messages, bot_user_id.as_deref());
    let thread_context_section = if thread_transcript.is_empty() {
        String::new()
    } else {
        format!("\n\nThread Context:\n{thread_transcript}")
    };

    format!("{task_prompt}{trigger_message_section}{thread_context_section}")
}

fn build_thread_transcript(
    messages: &[reili_core::messaging::slack::SlackThreadMessage],
    bot_user_id: Option<&str>,
) -> String {
    messages
        .iter()
        .map(|message| {
            let author = normalize_author(message.user.as_deref(), bot_user_id);
            let text = message.rendered_text();
            let text = text.trim();
            let iso_timestamp = to_iso_timestamp(&message.ts);
            format!(
                "ts: {}, iso_timestamp: {}, posted_by: {}\nmessage:{}",
                message.ts, iso_timestamp, author, text
            )
        })
        .collect::<Vec<String>>()
        .join("\n---\n")
}

fn normalize_author(user: Option<&str>, bot_user_id: Option<&str>) -> String {
    let normalized = match user.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value,
        None => return "system".to_string(),
    };

    if bot_user_id.is_some_and(|bot_user_id_value| normalized == bot_user_id_value) {
        return format!("{normalized} (You)");
    }

    normalized.to_string()
}

fn to_iso_timestamp(ts: &str) -> String {
    let mut parts = ts.split('.');
    let seconds_part = parts.next().unwrap_or_default();
    let milliseconds_part = parts.next().unwrap_or("0");

    let seconds = match seconds_part.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return "unknown".to_string(),
    };

    let milliseconds_slice = normalize_milliseconds(milliseconds_part);
    let milliseconds = match milliseconds_slice.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return "unknown".to_string(),
    };

    let unix_millis = match seconds
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(milliseconds))
    {
        Some(value) => value,
        None => return "unknown".to_string(),
    };

    match DateTime::<Utc>::from_timestamp_millis(unix_millis) {
        Some(value) => value.to_rfc3339_opts(SecondsFormat::Millis, true),
        None => "unknown".to_string(),
    }
}

fn normalize_milliseconds(milliseconds_part: &str) -> String {
    let mut normalized = milliseconds_part.to_string();
    while normalized.len() < 3 {
        normalized.push('0');
    }

    normalized.chars().take(3).collect()
}

fn extract_mentioned_user_id(text: &str) -> Option<String> {
    let start_index = text.find("<@")?;
    let remaining = &text[start_index + 2..];
    let end_index = remaining.find('>')?;
    let user_id = &remaining[..end_index];
    if user_id.is_empty() {
        return None;
    }

    if !user_id
        .chars()
        .all(|value| value.is_ascii_uppercase() || value.is_ascii_digit())
    {
        return None;
    }

    Some(user_id.to_string())
}

struct BuildSpecialistAgentInput<C>
where
    C: CompletionClient,
{
    client: C,
    settings: LlmProviderSettings,
    resources: Arc<TaskResources>,
    datadog_mcp_toolset: DatadogMcpToolset,
    github_mcp_toolset: GitHubMcpToolset,
    github_scope_org: String,
    language: String,
    additional_system_prompt: Option<String>,
    logger: Arc<dyn Logger>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    owner_id: String,
    runtime: TaskRuntime,
    cancellation: TaskCancellation,
    usage_collector: LlmUsageCollector,
}

fn build_datadog_agent<C>(input: BuildSpecialistAgentInput<C>) -> SpecialistAgent<C>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    let builder = input
        .client
        .agent(input.settings.specialist_model.clone())
        .name("investigate_datadog")
        .description("Delegates Datadog logs, metrics, and events investigation tasks.")
        .preamble(&build_datadog_instructions(BuildDatadogInstructionsInput {
            language: input.language,
            additional_system_prompt: input.additional_system_prompt,
        }))
        .default_max_turns(input.settings.specialist_max_turns)
        .additional_params(input.settings.additional_params.clone());
    let builder = with_max_tokens(builder, input.settings.max_tokens);

    builder
        .hook(AgentExecutionHook::new(
            input.owner_id.clone(),
            input.runtime,
            input.cancellation,
            input.logger,
            Arc::clone(&input.on_progress_event),
            input.usage_collector,
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tools(input.datadog_mcp_toolset.specialist_tools())
        .tool(SearchWebTool::new(input.resources))
        .build()
}

fn build_github_agent<C>(input: BuildSpecialistAgentInput<C>) -> SpecialistAgent<C>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    let builder = input
        .client
        .agent(input.settings.specialist_model.clone())
        .name("investigate_github")
        .description("Delegates GitHub repository, code, and pull request investigation tasks.")
        .preamble(&build_github_instructions(BuildGithubInstructionsInput {
            language: input.language,
            github_scope_org: input.github_scope_org.clone(),
            additional_system_prompt: input.additional_system_prompt,
        }))
        .default_max_turns(input.settings.specialist_max_turns)
        .additional_params(input.settings.additional_params.clone());
    let builder = with_max_tokens(builder, input.settings.max_tokens);
    let builder = builder
        .hook(AgentExecutionHook::new(
            input.owner_id.clone(),
            input.runtime,
            input.cancellation,
            input.logger,
            Arc::clone(&input.on_progress_event),
            input.usage_collector,
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tools(input.github_mcp_toolset.specialist_tools())
        .tool(SearchWebTool::new(input.resources));

    builder.build()
}

fn with_max_tokens<M, H>(
    builder: rig::agent::AgentBuilder<M, H>,
    max_tokens: Option<u64>,
) -> rig::agent::AgentBuilder<M, H>
where
    M: rig::completion::CompletionModel,
    H: rig::agent::PromptHook<M>,
{
    match max_tokens {
        Some(value) => builder.max_tokens(value),
        None => builder,
    }
}

struct BuildTaskInstructionsInput {
    datadog_site: String,
    github_scope_org: String,
    runtime: TaskRuntime,
    language: String,
    additional_system_prompt: Option<String>,
}

fn build_task_instructions(input: BuildTaskInstructionsInput) -> String {
    let datadog_site = if input.datadog_site.is_empty() {
        "datadoghq.com".to_string()
    } else {
        input.datadog_site
    };

    append_configured_additional_system_prompt(
        format!(
        "You are an expert SRE agent operating from Slack mentions,
with deep expertise in reliability, security, software development, and production operations.

Output language: {language}
- Use {language} for all responses and reasoning.

Current run context:
- Now: {now}
- StartedAt: {started_at_iso}
- Slack Channel: {channel}
- Slack Thread: {thread_ts}
- Retry Count: {retry_count}
- GitHub Organization Scope: {github_scope_org}
- Datadog Site: {datadog_site}

You orchestrate SRE work end-to-end, including investigation, operational support, diagnostics, direct retrieval, change assessment, and production guidance.
- First classify whether the request is incident investigation, operational support, direct retrieval, or another SRE task.
- Before entering a new major step, call report_progress.
- report_progress payload must be short and use title and summary fields.
- Your response is posted to Slack as-is.
- Write the final response as a concise, scannable Slack message using Slack markdown.
- For requests involving production systems, first build enough context to understand affected services, dependencies, recent changes, and operational risk before taking deeper action.
- Use Datadog MCP tools such as search_datadog_services, search_datadog_metrics, get_datadog_metric_context, and search_datadog_monitors early when they help establish service scope, system behavior, or alert context.
- Delegate detailed Datadog work to investigate_datadog and GitHub work to investigate_github as needed.
- Use GitHub context not only for investigations, but also for system understanding, ownership, recent changes, deployment context, and operational runbooks.
- Run independent tool calls in parallel where possible.
- Use search_slack_messages when prior Slack discussion outside the current thread could clarify timelines, alerts, ownership, or prior investigation notes.

Web search:
- Use search_web to check whether external dependencies (cloud providers, third-party APIs, SaaS platforms) are experiencing outages or degraded performance that could explain the symptoms observed internally.
- When internal metrics or logs suggest connectivity issues, elevated error rates toward external endpoints, or timeouts on third-party calls, proactively search for recent public incident reports or status page updates for those services.

Final answer requirements:
- Match the final response to the task type.
- For investigation tasks, present the strongest findings, likely causes, or relevant hypotheses, with supporting evidence and an explicit confidence level for each.
- Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns.
- Whenever Datadog, GitHub, Slack, documentation, or any other evidence source is referenced, include the supporting URL and format it as a clickable link in the Slack message.
- When useful, end with a brief recommended next step.
- Minimize emoji usage. Use emojis only when they add meaningful signal, and never as decoration.
- Keep the final response concise, scannable, and ready to post to Slack as-is.
- For direct retrieval, status checks, or simple operational tasks, provide a concise direct answer with only the minimum necessary context.",
        language = input.language,
        now = Utc::now().to_rfc3339(),
        started_at_iso = input.runtime.started_at_iso,
        channel = input.runtime.channel,
        thread_ts = input.runtime.thread_ts,
        retry_count = input.runtime.retry_count,
        github_scope_org = input.github_scope_org,
        datadog_site = datadog_site,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

struct BuildDatadogInstructionsInput {
    language: String,
    additional_system_prompt: Option<String>,
}

fn build_datadog_instructions(input: BuildDatadogInstructionsInput) -> String {
    append_configured_additional_system_prompt(
        format!(
        "You are a Datadog investigation specialist with deep expertise in production reliability, observability, incident analysis, and operational diagnostics.
Your role is to investigate Datadog evidence across logs, metrics, and events, and return concise, evidence-based findings that support safe and reliable operational decisions.

Use {language} for all responses.

## Investigation approach
Work in a hypothesis-driven way. Start by narrowing the service, timeframe, and current working hypothesis, then use only the Datadog tools needed to test that hypothesis or answer the current question. Prefer focused investigation over broad data collection.
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.

## Datadog usage
Use `search_datadog_logs` and `analyze_datadog_logs` to identify error patterns, anomalies, recurring messages, affected requests, and timeline clusters.
Use `search_datadog_metrics`, `get_datadog_metric`, and `get_datadog_metric_context` to inspect trends, spikes, regressions, saturation, and relevant dimensions such as env, region, version, endpoint, or dependency.
Use `search_datadog_events` to correlate deployments, incidents, monitor transitions, configuration changes, and other operational events.
Combine logs, metrics, and events when it materially improves confidence, clarifies scope, or helps rule out competing explanations.
Run independent tool calls in parallel when possible. If you receive `client_error` payloads or weak results, refine the query and retry when useful.

## Output expectations
Prioritize the most operationally relevant questions first: customer impact, affected scope, onset time, likely trigger, severity, and whether the issue is ongoing.
Return concise, high-signal findings rather than raw tool output. Clearly distinguish confirmed facts, plausible explanations, and remaining unknowns. Avoid overstating conclusions, and state uncertainty explicitly when evidence is partial, indirect, or conflicting.
Include clickable Datadog links for all referenced evidence whenever available, and structure findings so they can be reused directly in a Slack response."
            ,
            language = input.language,
        ),
        input.additional_system_prompt.as_deref(),
    )
}

struct BuildGithubInstructionsInput {
    language: String,
    github_scope_org: String,
    additional_system_prompt: Option<String>,
}

fn build_github_instructions(input: BuildGithubInstructionsInput) -> String {
    append_configured_additional_system_prompt(
        format!(
            "You are a GitHub analysis specialist with deep expertise in software development, production operations, repository analysis, and change investigation. Your role is to use GitHub evidence to clarify system structure, ownership, code behavior, recent changes, pull request context, and other repository facts that matter for safe and reliable operational decisions.

Use {language} for all responses.

## Working style
Before entering a new major investigation step, call report_progress. The payload must be short and use the title and summary fields.
Work in a focused, question-driven way. Use the available GitHub MCP tools to search code, repositories, issues, pull requests, and repository files, but only to the extent needed to answer the current question or test the current hypothesis. Run independent searches in parallel when possible.

## Mandatory scope rules
Every `search_code`, `search_repositories`, `search_issues`, and `search_pull_requests` call must include `org:{github_scope_org}`.
For `get_file_contents` and `pull_request_read`, the `owner` must be `{github_scope_org}`.
Never omit the org qualifier, switch owners, or access repositories outside `{github_scope_org}`.

## What to prioritize
Prioritize repository evidence that helps explain operational behavior, ownership, recent changes, deployment context, configuration, architecture, runbooks, and likely production impact.
When searching code, prefer identifiers, service names, alert names, config keys, endpoints, runbooks, infrastructure files, and operationally meaningful paths over generic keywords. When reviewing pull requests or issues, focus on recent changes, intended behavior, rollout context, known risks, follow-up discussion, and possible regressions.
When reading files, extract only the minimum necessary context needed to answer accurately. Prefer concise summaries over large excerpts.

## Evidence and output quality
Return concise, evidence-based findings rather than raw search output. Clearly distinguish confirmed facts, plausible inferences, and remaining unknowns. Avoid overstating conclusions when repository evidence is partial, indirect, or ambiguous.
Whenever you reference GitHub evidence, include the supporting GitHub URL as a clickable link whenever available. Structure findings so they can be reused directly in a Slack response by another agent.",
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
    use reili_core::messaging::slack::{
        SlackMessage, SlackMessageFile, SlackThreadMessage, SlackTriggerType,
    };
    use reili_core::task::TaskRequest;
    use reili_core::task::TaskRuntime;
    use serde_json::json;

    use super::super::llm_provider_settings::{
        CreateAnthropicProviderSettingsInput, CreateOpenAiProviderSettingsInput,
        create_anthropic_provider_settings, create_openai_provider_settings,
    };
    use super::{
        BuildDatadogInstructionsInput, BuildGithubInstructionsInput, BuildTaskInstructionsInput,
        build_datadog_instructions, build_github_instructions, build_task_instructions,
        build_task_prompt,
    };

    fn sample_trigger_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "Please investigate this alert".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    #[test]
    fn builds_task_prompt_with_thread_context() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000001".to_string(),
                user: Some("U123".to_string()),
                text: "thread context".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            }],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("Trigger Message: Please investigate this alert"));
        assert!(prompt.contains("Thread Context:"));
        assert!(prompt.contains("posted_by: U123"));
        assert!(prompt.contains("message:thread context"));
    }

    #[test]
    fn builds_task_prompt_without_thread_context() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("Trigger Message: Please investigate this alert"));
        assert!(!prompt.contains("Thread Context:"));
    }

    #[test]
    fn builds_task_prompt_with_bot_user_you_annotation() {
        let mut trigger = sample_trigger_message();
        trigger.text = "<@U999> investigate this alert".to_string();
        let request = TaskRequest {
            trigger_message: trigger,
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000010".to_string(),
                user: Some("U999".to_string()),
                text: "I started investigation".to_string(),
                legacy_attachments: Vec::new(),
                files: Vec::new(),
                metadata: None,
            }],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains("posted_by: U999 (You)"));
        assert!(prompt.contains("message:I started investigation"));
    }

    #[test]
    fn formats_thread_messages_as_transcript() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U123".to_string()),
                    text: "First message".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: None,
                    text: " follow-up from bot ".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
            ],
        };
        let prompt = build_task_prompt(&request);
        assert!(prompt.contains(
            "ts: 1710000000.000001, iso_timestamp: 2024-03-09T16:00:00.000Z, posted_by: U123\nmessage:First message"
        ));
        assert!(prompt.contains(
            "ts: 1710000000.000002, iso_timestamp: 2024-03-09T16:00:00.000Z, posted_by: system\nmessage:follow-up from bot"
        ));
    }

    #[test]
    fn includes_trigger_message_file_plain_text_in_prompt() {
        let mut trigger = sample_trigger_message();
        trigger.text = String::new();
        trigger.files = vec![SlackMessageFile {
            name: Some("aws-health.eml".to_string()),
            title: Some("AWS Health Event".to_string()),
            plain_text: Some("scheduled upgrade required".to_string()),
        }];
        let request = TaskRequest {
            trigger_message: trigger,
            thread_messages: vec![],
        };

        let prompt = build_task_prompt(&request);

        assert!(prompt.contains("Trigger Message: attached_file: aws-health.eml"));
        assert!(prompt.contains("plain_text:\nscheduled upgrade required"));
    }

    #[test]
    fn includes_thread_message_file_plain_text_in_prompt() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000002".to_string(),
                user: Some("U123".to_string()),
                text: String::new(),
                legacy_attachments: Vec::new(),
                files: vec![SlackMessageFile {
                    name: Some("aws-health.eml".to_string()),
                    title: Some("AWS Health Event".to_string()),
                    plain_text: Some("scheduled upgrade required".to_string()),
                }],
                metadata: None,
            }],
        };

        let prompt = build_task_prompt(&request);

        assert!(prompt.contains("posted_by: U123"));
        assert!(prompt.contains("message:attached_file: aws-health.eml"));
        assert!(prompt.contains("plain_text:\nscheduled upgrade required"));
    }

    #[test]
    fn provider_settings_enable_parallel_tool_calls() {
        let settings = create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
            model: "gpt-5.3-codex".to_string(),
        });
        let params = settings.additional_params;

        assert_eq!(
            params.get("parallel_tool_calls"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            params.get("text"),
            Some(&json!({ "format": { "type": "text" } }))
        );
    }

    #[test]
    fn anthropic_provider_settings_assign_max_tokens_for_supported_models() {
        let cases = [
            ("claude-opus-4-6", Some(32_000)),
            ("claude-sonnet-4-6", Some(64_000)),
            ("claude-haiku-4-5", Some(4_096)),
        ];

        for (model, expected_max_tokens) in cases {
            let settings =
                create_anthropic_provider_settings(CreateAnthropicProviderSettingsInput {
                    model: model.to_string(),
                });

            assert_eq!(settings.max_tokens, expected_max_tokens);
        }
    }

    #[test]
    fn task_instructions_include_report_progress_rules() {
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

        assert!(instructions.contains("call report_progress"));
        assert!(instructions.contains("title and summary fields"));
        assert!(instructions.contains("Do not post consecutive report_progress"));
        assert!(instructions.contains("search_datadog_services"));
        assert!(instructions.contains("search_datadog_metrics"));
        assert!(instructions.contains("get_datadog_metric_context"));
        assert!(instructions.contains("investigate_datadog"));
        assert!(
            !instructions.contains("investigate_logs / investigate_metrics / investigate_events")
        );
    }

    #[test]
    fn specialist_instructions_include_report_progress_rules() {
        let datadog_instructions = build_datadog_instructions(BuildDatadogInstructionsInput {
            language: "Japanese".to_string(),
            additional_system_prompt: None,
        });
        let github_instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
            additional_system_prompt: None,
        });

        for instructions in [datadog_instructions, github_instructions.clone()] {
            assert!(instructions.contains("call report_progress"));
            assert!(instructions.contains("title and summary fields"));
            assert!(instructions.contains("Do not post consecutive report_progress"));
        }

        assert!(
            build_datadog_instructions(BuildDatadogInstructionsInput {
                language: "Japanese".to_string(),
                additional_system_prompt: None,
            })
            .contains("analyze_datadog_logs")
        );
        assert!(
            build_datadog_instructions(BuildDatadogInstructionsInput {
                language: "Japanese".to_string(),
                additional_system_prompt: None,
            })
            .contains("get_datadog_metric")
        );
        assert!(
            build_datadog_instructions(BuildDatadogInstructionsInput {
                language: "Japanese".to_string(),
                additional_system_prompt: None,
            })
            .contains("get_datadog_metric_context")
        );
        assert!(
            build_datadog_instructions(BuildDatadogInstructionsInput {
                language: "Japanese".to_string(),
                additional_system_prompt: None,
            })
            .contains("search_datadog_events")
        );
        assert!(
            build_datadog_instructions(BuildDatadogInstructionsInput {
                language: "Japanese".to_string(),
                additional_system_prompt: None,
            })
            .contains("Inspect logs")
        );
        assert!(
            github_instructions
                .contains("search_code/search_repositories/search_issues/search_pull_requests")
        );
        assert!(github_instructions.contains("get_file_contents"));
        assert!(github_instructions.contains("pull_request_read"));
    }

    #[test]
    fn task_instructions_include_web_search_rules() {
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

        assert!(instructions.contains("search_web"));
        assert!(instructions.contains("search_slack_messages"));
        assert!(instructions.contains("external dependencies"));
        assert!(instructions.contains("public incident reports or status page"));
    }

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
}
