use std::sync::Arc;

use chrono::Utc;
use reili_shared::ports::outbound::{
    COORDINATOR_PROGRESS_OWNER_ID, InvestigationProgressEventPort, InvestigationResources,
    InvestigationRuntime,
};
use reili_shared::types::AlertContext;
use rig::agent::Agent;
use rig::prelude::CompletionClient;
use rig::providers::openai;
use serde_json::json;

use super::tools::{
    AggregateDatadogLogsByFacetTool, GetPullRequestDiffTool, GetPullRequestTool,
    GetRepositoryContentTool, ListDatadogMetricsCatalogTool, QueryDatadogMetricsTool,
    ReportProgressTool, ReportProgressToolInput, SearchDatadogEventsTool, SearchDatadogLogsTool,
    SearchGithubCodeTool, SearchGithubIssuesAndPullRequestsTool, SearchGithubReposTool,
    SearchWebTool,
};
use super::{
    progress_event_hook::ProgressEventHook,
    progress_reporting_sub_agent_tool::ProgressReportingSubAgentTool,
};

const INVESTIGATION_MODEL: &str = "gpt-5.3-codex";
const COORDINATOR_MAX_TURNS: usize = 20;
const SUBAGENT_MAX_TURNS: usize = 50;
const LOGS_PROGRESS_OWNER_ID: &str = "investigate_logs";
const METRICS_PROGRESS_OWNER_ID: &str = "investigate_metrics";
const EVENTS_PROGRESS_OWNER_ID: &str = "investigate_events";
const GITHUB_PROGRESS_OWNER_ID: &str = "investigate_github";

type OpenAiCompletionModel = <openai::Client as CompletionClient>::CompletionModel;
type OpenAiAgent = Agent<OpenAiCompletionModel>;
type OpenAiSubAgent = Agent<OpenAiCompletionModel, ProgressEventHook>;

pub struct BuildCoordinatorAgentInput {
    pub client: openai::Client,
    pub resources: Arc<InvestigationResources>,
    pub datadog_site: String,
    pub github_scope_org: String,
    pub runtime: InvestigationRuntime,
    pub on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    pub language: String,
}

#[must_use]
pub fn build_coordinator_agent(input: BuildCoordinatorAgentInput) -> OpenAiAgent {
    let logs_agent = build_logs_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        resources: Arc::clone(&input.resources),
        github_scope_org: String::new(),
        language: input.language.clone(),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: LOGS_PROGRESS_OWNER_ID.to_string(),
    });
    let metrics_agent = build_metrics_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        resources: Arc::clone(&input.resources),
        github_scope_org: String::new(),
        language: input.language.clone(),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: METRICS_PROGRESS_OWNER_ID.to_string(),
    });
    let events_agent = build_events_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        resources: Arc::clone(&input.resources),
        github_scope_org: String::new(),
        language: input.language.clone(),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: EVENTS_PROGRESS_OWNER_ID.to_string(),
    });
    let github_agent = build_github_agent(BuildSpecialistAgentInput {
        client: input.client.clone(),
        resources: Arc::clone(&input.resources),
        github_scope_org: input.github_scope_org.clone(),
        language: input.language.clone(),
        on_progress_event: Arc::clone(&input.on_progress_event),
        owner_id: GITHUB_PROGRESS_OWNER_ID.to_string(),
    });

    input
        .client
        .agent(INVESTIGATION_MODEL)
        .name("Coordinator")
        .preamble(&build_coordinator_instructions(
            BuildCoordinatorInstructionsInput {
                datadog_site: input.datadog_site,
                github_scope_org: input.github_scope_org,
                runtime: input.runtime,
                language: input.language,
            },
        ))
        .default_max_turns(COORDINATOR_MAX_TURNS)
        .additional_params(model_additional_params())
        .tool(AggregateDatadogLogsByFacetTool::new(Arc::clone(
            &input.resources,
        )))
        .tool(ListDatadogMetricsCatalogTool::new(Arc::clone(
            &input.resources,
        )))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: COORDINATOR_PROGRESS_OWNER_ID.to_string(),
        }))
        .tool(ProgressReportingSubAgentTool::new(
            logs_agent,
            LOGS_PROGRESS_OWNER_ID.to_string(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(ProgressReportingSubAgentTool::new(
            metrics_agent,
            METRICS_PROGRESS_OWNER_ID.to_string(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(ProgressReportingSubAgentTool::new(
            events_agent,
            EVENTS_PROGRESS_OWNER_ID.to_string(),
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
pub fn build_coordinator_prompt(alert_context: &AlertContext) -> String {
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
struct BuildSpecialistAgentInput {
    client: openai::Client,
    resources: Arc<InvestigationResources>,
    github_scope_org: String,
    language: String,
    on_progress_event: Arc<dyn InvestigationProgressEventPort>,
    owner_id: String,
}

fn build_logs_agent(input: BuildSpecialistAgentInput) -> OpenAiSubAgent {
    input
        .client
        .agent(INVESTIGATION_MODEL)
        .name("investigate_logs")
        .description("Delegates Datadog log investigation tasks.")
        .preamble(&build_logs_instructions(&input.language))
        .default_max_turns(SUBAGENT_MAX_TURNS)
        .additional_params(model_additional_params())
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tool(SearchDatadogLogsTool::new(Arc::clone(&input.resources)))
        .tool(SearchWebTool::new(input.resources))
        .build()
}

fn build_metrics_agent(input: BuildSpecialistAgentInput) -> OpenAiSubAgent {
    input
        .client
        .agent(INVESTIGATION_MODEL)
        .name("investigate_metrics")
        .description("Delegates Datadog metrics investigation tasks.")
        .preamble(&build_metrics_instructions(&input.language))
        .default_max_turns(SUBAGENT_MAX_TURNS)
        .additional_params(model_additional_params())
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tool(QueryDatadogMetricsTool::new(Arc::clone(&input.resources)))
        .tool(SearchWebTool::new(input.resources))
        .build()
}

fn build_events_agent(input: BuildSpecialistAgentInput) -> OpenAiSubAgent {
    input
        .client
        .agent(INVESTIGATION_MODEL)
        .name("investigate_events")
        .description("Delegates Datadog event investigation tasks.")
        .preamble(&build_events_instructions(&input.language))
        .default_max_turns(SUBAGENT_MAX_TURNS)
        .additional_params(model_additional_params())
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
            Arc::clone(&input.on_progress_event),
        ))
        .tool(ReportProgressTool::new(ReportProgressToolInput {
            on_progress_event: Arc::clone(&input.on_progress_event),
            owner_id: input.owner_id,
        }))
        .tool(SearchDatadogEventsTool::new(Arc::clone(&input.resources)))
        .tool(SearchWebTool::new(input.resources))
        .build()
}

fn build_github_agent(input: BuildSpecialistAgentInput) -> OpenAiSubAgent {
    input
        .client
        .agent(INVESTIGATION_MODEL)
        .name("investigate_github")
        .description("Delegates GitHub repository, code, and pull request investigation tasks.")
        .preamble(&build_github_instructions(BuildGithubInstructionsInput {
            language: input.language,
            github_scope_org: input.github_scope_org.clone(),
        }))
        .default_max_turns(SUBAGENT_MAX_TURNS)
        .additional_params(model_additional_params())
        .hook(ProgressEventHook::new(
            input.owner_id.clone(),
            Arc::clone(&input.on_progress_event),
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
        .tool(GetRepositoryContentTool::new(
            Arc::clone(&input.resources.github_repository_content_port),
            input.github_scope_org.clone(),
        ))
        .tool(GetPullRequestTool::new(
            Arc::clone(&input.resources.github_pull_request_port),
            input.github_scope_org.clone(),
        ))
        .tool(GetPullRequestDiffTool::new(
            Arc::clone(&input.resources.github_pull_request_port),
            input.github_scope_org,
        ))
        .tool(SearchWebTool::new(input.resources))
        .build()
}

#[must_use]
fn model_additional_params() -> serde_json::Value {
    json!({
        "reasoning": {
            "effort": "low",
            "summary": "auto",
        },
        "text": {
            "format": {
                "type": "text",
            },
        },
        "parallel_tool_calls": true,
    })
}

struct BuildCoordinatorInstructionsInput {
    datadog_site: String,
    github_scope_org: String,
    runtime: InvestigationRuntime,
    language: String,
}

fn build_coordinator_instructions(input: BuildCoordinatorInstructionsInput) -> String {
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
- Use aggregate_datadog_logs_by_facet and list_datadog_metrics_catalog early to understand service scope.
- Delegate detailed work to investigate_logs / investigate_metrics / investigate_events / investigate_github as needed.
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

fn build_logs_instructions(language: &str) -> String {
    format!(
        "You are a log analysis specialist.
Before entering a new investigation step, call report_progress.
report_progress payload must be short: use title and summary fields.
Do not post consecutive report_progress calls with identical content.
Use search_datadog_logs and summarize errors, anomalies, and patterns.
Run independent tool calls in parallel when possible.
If you receive client_error payloads, adjust query and retry when useful.
Use {language} for all responses."
    )
}

fn build_metrics_instructions(language: &str) -> String {
    format!(
        "You are a metrics analysis specialist.
Before entering a new investigation step, call report_progress.
report_progress payload must be short: use title and summary fields.
Do not post consecutive report_progress calls with identical content.
Use query_datadog_metrics and summarize trends, spikes, and anomalies.
Run independent tool calls in parallel when possible.
If you receive client_error payloads, adjust query and retry when useful.
Use {language} for all responses."
    )
}

fn build_events_instructions(language: &str) -> String {
    format!(
        "You are an events analysis specialist.
Before entering a new investigation step, call report_progress.
report_progress payload must be short: use title and summary fields.
Do not post consecutive report_progress calls with identical content.
Use search_datadog_events and correlate deployments/config changes with incidents.
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
    use reili_shared::ports::outbound::InvestigationRuntime;
    use reili_shared::types::AlertContext;
    use serde_json::json;

    use super::{
        BuildCoordinatorInstructionsInput, BuildGithubInstructionsInput,
        build_coordinator_instructions, build_coordinator_prompt, build_events_instructions,
        build_github_instructions, build_logs_instructions, build_metrics_instructions,
        model_additional_params,
    };

    fn sample_alert_context() -> AlertContext {
        AlertContext {
            raw_text: "Please investigate this alert".to_string(),
            trigger_message_text: "Please investigate this alert".to_string(),
            thread_transcript: "thread context".to_string(),
        }
    }

    #[test]
    fn builds_coordinator_prompt_with_thread_context() {
        let prompt = build_coordinator_prompt(&sample_alert_context());
        assert!(prompt.contains("Trigger Message: Please investigate this alert"));
        assert!(prompt.contains("Thread Context:\nthread context"));
    }

    #[test]
    fn model_additional_params_enable_parallel_tool_calls() {
        let params = model_additional_params();
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
    fn coordinator_instructions_include_report_progress_rules() {
        let instructions = build_coordinator_instructions(BuildCoordinatorInstructionsInput {
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
    }

    #[test]
    fn specialist_instructions_include_report_progress_rules() {
        let logs_instructions = build_logs_instructions("Japanese");
        let metrics_instructions = build_metrics_instructions("Japanese");
        let events_instructions = build_events_instructions("Japanese");
        let github_instructions = build_github_instructions(BuildGithubInstructionsInput {
            language: "Japanese".to_string(),
            github_scope_org: "acme".to_string(),
        });

        for instructions in [
            logs_instructions,
            metrics_instructions,
            events_instructions,
            github_instructions,
        ] {
            assert!(instructions.contains("call report_progress"));
            assert!(instructions.contains("title and summary fields"));
            assert!(instructions.contains("Do not post consecutive report_progress"));
        }
    }

    #[test]
    fn coordinator_instructions_include_web_search_rules() {
        let instructions = build_coordinator_instructions(BuildCoordinatorInstructionsInput {
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
