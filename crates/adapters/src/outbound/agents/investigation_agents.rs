use std::sync::Arc;

use chrono::Utc;
use reili_core::investigation::AlertContext;
use reili_core::investigation::{
    INVESTIGATION_LEAD_PROGRESS_OWNER_ID, InvestigationProgressEventPort, InvestigationResources,
    InvestigationRuntime,
};
use rig::agent::Agent;
use rig::prelude::CompletionClient;

use super::datadog_mcp_tools::DatadogMcpToolset;
use super::tools::{
    GetPullRequestDiffTool, GetPullRequestTool, GetRepositoryContentTool, ReportProgressTool,
    ReportProgressToolInput, SearchGithubCodeTool, SearchGithubIssuesAndPullRequestsTool,
    SearchGithubReposTool, SearchWebTool,
};
use super::{
    llm_provider_settings::LlmProviderSettings, llm_usage_collector::LlmUsageCollector,
    progress_event_hook::ProgressEventHook,
    progress_reporting_sub_agent_tool::ProgressReportingSubAgentTool,
};

const DATADOG_PROGRESS_OWNER_ID: &str = "investigate_datadog";
const GITHUB_PROGRESS_OWNER_ID: &str = "investigate_github";

type CompletionAgent<C> = Agent<<C as CompletionClient>::CompletionModel>;
type SpecialistAgent<C> = Agent<<C as CompletionClient>::CompletionModel, ProgressEventHook>;

pub struct BuildInvestigationLeadAgentInput<C>
where
    C: CompletionClient,
{
    pub client: C,
    pub settings: LlmProviderSettings,
    pub resources: Arc<InvestigationResources>,
    pub datadog_site: String,
    pub datadog_mcp_toolset: DatadogMcpToolset,
    pub github_scope_org: String,
    pub runtime: InvestigationRuntime,
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    pub language: String,
    pub usage_collector: LlmUsageCollector,
}

#[must_use]
pub fn build_investigation_lead_agent<C>(
    input: BuildInvestigationLeadAgentInput<C>,
) -> CompletionAgent<C>
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
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: DATADOG_PROGRESS_OWNER_ID.to_string(),
        usage_collector: input.usage_collector.clone(),
    });
    let github_agent = build_github_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        settings: input.settings.clone(),
        resources: Arc::clone(&input.resources),
        datadog_mcp_toolset: input.datadog_mcp_toolset.clone(),
        github_scope_org: input.github_scope_org.clone(),
        language: input.language.clone(),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: GITHUB_PROGRESS_OWNER_ID.to_string(),
        usage_collector: input.usage_collector.clone(),
    });

    input
        .client
        .agent(input.settings.investigation_lead_model.clone())
        .name("InvestigationLead")
        .preamble(&build_investigation_lead_instructions(
            BuildInvestigationLeadInstructionsInput {
                datadog_site: input.datadog_site,
                github_scope_org: input.github_scope_org,
                runtime: input.runtime,
                language: input.language,
            },
        ))
        .default_max_turns(input.settings.investigation_lead_max_turns)
        .additional_params(input.settings.additional_params.clone())
        .tools(input.datadog_mcp_toolset.lead_tools())
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: INVESTIGATION_LEAD_PROGRESS_OWNER_ID.to_string(),
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
pub fn build_investigation_lead_prompt(alert_context: &AlertContext) -> String {
    let investigation_prompt = "Investigate the following user input and respond with the most appropriate investigation or direct answer.
The input may be an alert, request, question, link, or partial context.";
    let trigger_message_section = format!(
        "\n\nTrigger Message: {}",
        alert_context.trigger_message_text
    );
    let thread_context_section = if alert_context.thread_transcript.is_empty() {
        String::new()
    } else {
        format!("\n\nThread Context:\n{}", alert_context.thread_transcript)
    };

    format!("{investigation_prompt}{trigger_message_section}{thread_context_section}")
}
struct BuildSpecialistAgentInput<C>
where
    C: CompletionClient,
{
    client: C,
    settings: LlmProviderSettings,
    resources: Arc<InvestigationResources>,
    datadog_mcp_toolset: DatadogMcpToolset,
    github_scope_org: String,
    language: String,
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    owner_id: String,
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
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
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
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
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

struct BuildInvestigationLeadInstructionsInput {
    datadog_site: String,
    github_scope_org: String,
    runtime: InvestigationRuntime,
    language: String,
}

fn build_investigation_lead_instructions(input: BuildInvestigationLeadInstructionsInput) -> String {
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
    use reili_core::investigation::AlertContext;
    use reili_core::investigation::InvestigationRuntime;
    use serde_json::json;

    use super::super::llm_provider_settings::{
        CreateOpenAiProviderSettingsInput, create_openai_provider_settings,
    };
    use super::{
        BuildGithubInstructionsInput, BuildInvestigationLeadInstructionsInput,
        build_datadog_instructions, build_github_instructions,
        build_investigation_lead_instructions, build_investigation_lead_prompt,
    };

    fn sample_alert_context() -> AlertContext {
        AlertContext {
            raw_text: "Please investigate this alert".to_string(),
            trigger_message_text: "Please investigate this alert".to_string(),
            thread_transcript: "thread context".to_string(),
        }
    }

    #[test]
    fn builds_investigation_lead_prompt_with_thread_context() {
        let prompt = build_investigation_lead_prompt(&sample_alert_context());
        assert!(prompt.contains("Trigger Message: Please investigate this alert"));
        assert!(prompt.contains("Thread Context:\nthread context"));
    }

    #[test]
    fn provider_settings_enable_parallel_tool_calls() {
        let settings = create_openai_provider_settings(CreateOpenAiProviderSettingsInput {
            investigation_lead_model: "gpt-5.3-codex".to_string(),
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
    fn investigation_lead_instructions_include_report_progress_rules() {
        let instructions =
            build_investigation_lead_instructions(BuildInvestigationLeadInstructionsInput {
                datadog_site: "datadoghq.com".to_string(),
                github_scope_org: "acme".to_string(),
                runtime: InvestigationRuntime {
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
    fn investigation_lead_instructions_include_web_search_rules() {
        let instructions =
            build_investigation_lead_instructions(BuildInvestigationLeadInstructionsInput {
                datadog_site: "datadoghq.com".to_string(),
                github_scope_org: "acme".to_string(),
                runtime: InvestigationRuntime {
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
