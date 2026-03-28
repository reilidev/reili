use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use reili_core::logger::Logger;
use reili_core::task::{
    TASK_RUNNER_PROGRESS_OWNER_ID, TaskProgressEventPort, TaskRequest, TaskResources, TaskRuntime,
};
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::agent_execution_hook::AgentExecutionHook;
use super::datadog_mcp_tools::DatadogMcpToolset;
use super::tools::{
    GetPullRequestDiffTool, GetPullRequestTool, GetRepositoryContentTool, ReportProgressTool,
    ReportProgressToolInput, SearchGithubCodeTool, SearchGithubIssuesAndPullRequestsTool,
    SearchGithubReposTool, SearchWebTool,
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
    pub github_scope_org: String,
    pub logger: Arc<dyn Logger>,
    pub runtime: TaskRuntime,
    pub on_progress_event: Arc<dyn TaskProgressEventPort>,
    pub language: String,
    pub usage_collector: LlmUsageCollector,
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
        github_scope_org: String::new(),
        language: input.language.clone(),
        logger: Arc::clone(&input.logger),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: DATADOG_PROGRESS_OWNER_ID.to_string(),
        runtime: input.runtime.clone(),
        usage_collector: input.usage_collector.clone(),
    });
    let github_agent = build_github_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        settings: input.settings.clone(),
        resources: Arc::clone(&input.resources),
        datadog_mcp_toolset: input.datadog_mcp_toolset.clone(),
        github_scope_org: input.github_scope_org.clone(),
        language: input.language.clone(),
        logger: Arc::clone(&input.logger),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: GITHUB_PROGRESS_OWNER_ID.to_string(),
        runtime: input.runtime.clone(),
        usage_collector: input.usage_collector.clone(),
    });

    input
        .client
        .agent(input.settings.task_runner_model.clone())
        .name("TaskRunner")
        .preamble(&build_task_instructions(BuildTaskInstructionsInput {
            datadog_site: input.datadog_site,
            github_scope_org: input.github_scope_org,
            runtime: input.runtime,
            language: input.language,
        }))
        .default_max_turns(input.settings.task_runner_max_turns)
        .additional_params(input.settings.additional_params.clone())
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
    let trigger_message_text = request.trigger_message.text.trim();
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
            let text = message.text.trim();
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
    github_scope_org: String,
    language: String,
    logger: Arc<dyn Logger>,
    on_progress_event: Arc<dyn TaskProgressEventPort>,
    owner_id: String,
    runtime: TaskRuntime,
    usage_collector: LlmUsageCollector,
}

fn build_datadog_agent<C>(input: BuildSpecialistAgentInput<C>) -> SpecialistAgent<C>
where
    C: CompletionClient,
    C::CompletionModel: 'static,
{
    input
        .client
        .agent(input.settings.specialist_model.clone())
        .name("investigate_datadog")
        .description("Delegates Datadog logs, metrics, and events investigation tasks.")
        .preamble(&build_datadog_instructions(&input.language))
        .default_max_turns(input.settings.specialist_max_turns)
        .additional_params(input.settings.additional_params.clone())
        .hook(AgentExecutionHook::new(
            input.owner_id.clone(),
            input.runtime,
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
    input
        .client
        .agent(input.settings.specialist_model.clone())
        .name("investigate_github")
        .description("Delegates GitHub repository, code, and pull request investigation tasks.")
        .preamble(&build_github_instructions(BuildGithubInstructionsInput {
            language: input.language,
            github_scope_org: input.github_scope_org.clone(),
        }))
        .default_max_turns(input.settings.specialist_max_turns)
        .additional_params(input.settings.additional_params.clone())
        .hook(AgentExecutionHook::new(
            input.owner_id.clone(),
            input.runtime,
            input.logger,
            Arc::clone(&input.on_progress_event),
            input.usage_collector,
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tool(SearchGithubCodeTool::new(
            Arc::clone(&input.resources.github_code_search_port),
            input.github_scope_org.clone(),
        ))
        .tool(SearchGithubReposTool::new(
            Arc::clone(&input.resources.github_code_search_port),
            input.github_scope_org.clone(),
        ))
        .tool(SearchGithubIssuesAndPullRequestsTool::new(
            Arc::clone(&input.resources.github_code_search_port),
            input.github_scope_org.clone(),
        ))
        .tool(GetRepositoryContentTool::new(Arc::clone(
            &input.resources.github_repository_content_port,
        )))
        .tool(GetPullRequestTool::new(Arc::clone(
            &input.resources.github_pull_request_port,
        )))
        .tool(GetPullRequestDiffTool::new(Arc::clone(
            &input.resources.github_pull_request_port,
        )))
        .tool(SearchWebTool::new(input.resources))
        .build()
}

struct BuildTaskInstructionsInput {
    datadog_site: String,
    github_scope_org: String,
    runtime: TaskRuntime,
    language: String,
}

fn build_task_instructions(input: BuildTaskInstructionsInput) -> String {
    let datadog_site = if input.datadog_site.is_empty() {
        "datadoghq.com".to_string()
    } else {
        input.datadog_site
    };

    format!(
        "You are an SRE/Security/Platform engineer operating from Slack mentions.

Output language: {language}
- Use {language} for all responses and reasoning.

Current run context:
- Now: {now}
- StartedAt: {started_at_iso}
- Slack Channel: {channel}
- Slack Thread: {thread_ts}
- Retry Count: {retry_count}
- GitHub Organization Scope: {github_scope_org}

You orchestrate investigation end-to-end.
- First classify if the request is incident investigation or direct retrieval.
- Before entering a new investigation step, call report_progress.
- report_progress payload must be short: use title and summary fields.
- Do not post consecutive report_progress calls with identical content.
- Your response is posted to Slack as-is. Do not rely on any downstream rewriting.
- Write the final response as a concise, scannable Slack message using Slack markdown.
- For investigation mode, establish system map from GitHub before deep Datadog querying.
- Use Datadog MCP tools such as search_datadog_services, search_datadog_metrics, get_datadog_metric_context, and search_datadog_monitors early to understand service scope.
- Delegate detailed Datadog work to investigate_datadog and GitHub work to investigate_github as needed.
- Run independent tool calls in parallel where possible.

Web search:
- Use search_web to check whether external dependencies (cloud providers, CDNs, DNS, third-party APIs, SaaS platforms) are experiencing outages or degraded performance that could explain the symptoms observed internally.
- When internal metrics or logs suggest connectivity issues, elevated error rates toward external endpoints, or timeouts on third-party calls, proactively search for recent public incident reports or status page updates for those services.

Final answer requirements:
- In investigation mode, provide several plausible findings with evidence and confidence.
- Include Datadog deep links with this site: {datadog_site}
- Include short sections: What I checked / What I did not find (if relevant).
- In direct task mode, provide concise direct answer with minimal extra content.",
        language = input.language,
        now = Utc::now().to_rfc3339(),
        started_at_iso = input.runtime.started_at_iso,
        channel = input.runtime.channel,
        thread_ts = input.runtime.thread_ts,
        retry_count = input.runtime.retry_count,
        github_scope_org = input.github_scope_org,
        datadog_site = datadog_site,
    )
}

fn build_datadog_instructions(language: &str) -> String {
    format!(
        "You are a Datadog investigation specialist covering logs, metrics, and events.
Before entering a new investigation step, call report_progress.
report_progress payload must be short: use title and summary fields.
Do not post consecutive report_progress calls with identical content.
Use explicit progress titles such as Inspect logs, Check metric spike, or Correlate events.
Start by narrowing the service, timeframe, and current hypothesis.
Use only the Datadog tools needed for the hypothesis you are testing.
Use search_datadog_logs and analyze_datadog_logs to summarize errors, anomalies, and patterns.
Use search_datadog_metrics, get_datadog_metric, and get_datadog_metric_context to inspect trends, spikes, and related dimensions.
Use search_datadog_events to correlate deployments, incidents, and configuration changes.
Combine logs, metrics, and events when it materially improves confidence.
Run independent tool calls in parallel when possible.
If you receive client_error payloads, adjust query and retry when useful.
Use {language} for all responses."
    )
}

struct BuildGithubInstructionsInput {
    language: String,
    github_scope_org: String,
}

fn build_github_instructions(input: BuildGithubInstructionsInput) -> String {
    format!(
        "You are a GitHub analysis specialist.
Before entering a new investigation step, call report_progress.
report_progress payload must be short: use title and summary fields.
Do not post consecutive report_progress calls with identical content.
Use the available tools to search code, repositories, issues, pull requests, and repository files.
Mandatory query rule:
- Every search_github_code/search_github_repos/search_github_issues_and_pull_requests call must include org:{github_scope_org}
- Never omit the org qualifier.
Run independent searches in parallel when possible.
Use {language} for all responses.",
        github_scope_org = input.github_scope_org,
        language = input.language,
    )
}

#[cfg(test)]
mod tests {
    use reili_core::messaging::slack::{SlackMessage, SlackThreadMessage, SlackTriggerType};
    use reili_core::task::TaskRequest;
    use reili_core::task::TaskRuntime;
    use serde_json::json;

    use super::super::llm_provider_settings::{
        CreateOpenAiProviderSettingsInput, create_openai_provider_settings,
    };
    use super::{
        BuildGithubInstructionsInput, BuildTaskInstructionsInput, build_datadog_instructions,
        build_github_instructions, build_task_instructions, build_task_prompt,
    };

    fn sample_trigger_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            text: "Please investigate this alert".to_string(),
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
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: None,
                    text: " follow-up from bot ".to_string(),
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
    fn provider_settings_enable_parallel_tool_calls() {
        let settings = create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
            task_runner_model: "gpt-5.3-codex".to_string(),
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
        let datadog_instructions = build_datadog_instructions("Japanese");
        let github_instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
        });

        for instructions in [datadog_instructions, github_instructions] {
            assert!(instructions.contains("call report_progress"));
            assert!(instructions.contains("title and summary fields"));
            assert!(instructions.contains("Do not post consecutive report_progress"));
        }

        assert!(build_datadog_instructions("Japanese").contains("analyze_datadog_logs"));
        assert!(build_datadog_instructions("Japanese").contains("get_datadog_metric"));
        assert!(build_datadog_instructions("Japanese").contains("get_datadog_metric_context"));
        assert!(build_datadog_instructions("Japanese").contains("search_datadog_events"));
        assert!(build_datadog_instructions("Japanese").contains("Inspect logs"));
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
        });

        assert!(instructions.contains("search_web"));
        assert!(instructions.contains("external dependencies"));
        assert!(instructions.contains("public incident reports or status page"));
    }
}
