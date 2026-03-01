import { Agent, type ModelSettings } from "@openai/agents";
import type {
	InvestigationContext,
	InvestigationRuntime,
} from "../../../ports/outbound/investigation-context";
import type { OpenAiSubAgentStreamCallbackFactory } from "./openai-subagent-stream-callback";
import { aggregateLogsByFacetTool } from "./tools/aggregate-datadog-logs-by-facet-tool";
import { getPullRequestDiffTool } from "./tools/get-pull-request-diff-tool";
import { getPullRequestTool } from "./tools/get-pull-request-tool";
import { getRepositoryContentTool } from "./tools/get-repository-content-tool";
import { listMetricsCatalogTool } from "./tools/list-datadog-metrics-catalog-tool";
import { queryMetricsTool } from "./tools/query-datadog-metrics-tool";
import { searchEventsTool } from "./tools/search-datadog-events-tool";
import { searchLogsTool } from "./tools/search-datadog-logs-tool";
import { createSearchGithubCodeTool } from "./tools/search-github-code-tool";
import { createSearchGithubIssuesAndPullRequestsTool } from "./tools/search-github-issues-and-pull-requests-tool";
import { createSearchGithubReposTool } from "./tools/search-github-repos-tool";

export type {
	InvestigationContext,
	InvestigationResources,
	InvestigationRuntime,
} from "../../../ports/outbound/investigation-context";

const INVESTIGATION_MODEL = "gpt-5.3-codex";

interface CoordinatorInstructionInput {
	datadogSite: string;
	githubScopeOrg: string;
	runtime: InvestigationRuntime;
	language: string;
}

interface AgentLanguageInput {
	language: string;
}

interface CreateCoordinatorAgentInput {
	logsAgent: Agent<InvestigationContext>;
	metricsAgent: Agent<InvestigationContext>;
	eventsAgent: Agent<InvestigationContext>;
	githubExplorerAgent: Agent<InvestigationContext>;
	onSubAgentStream?: OpenAiSubAgentStreamCallbackFactory;
	language: string;
}

export interface CreateSynthesizerAgentInput {
	language: string;
}

function buildCoordinatorInstructions(
	input: CoordinatorInstructionInput,
): string {
	return `
You are an SRE/Security/Platform engineer operating from Slack mentions. 

## Output Language
${input.language}

Language Policy:
- Use ${input.language} for all conversation and reasoning.
- Keep every response, plan, and analysis in ${input.language}.
- Do not switch languages unless explicitly requested by the user.

Current run context:
- Now: ${new Date().toISOString()}
- Slack Channel: ${input.runtime.channel}
- Slack Thread: ${input.runtime.threadTs}
- Retry Count: ${input.runtime.retryCount}
- GitHub Organization Scope: ${input.githubScopeOrg}

Your job is to orchestrate a thorough and fast investigation of requests that arrive via Slack mentions, such as:
- Another member’s investigation instruction (e.g., “please check this alert”, “triage this”, “estimate impact”, “find the likely cause”)
- Alert/notification investigation requests (full alert bodies, links, pasted logs, dashboards)
- Ad-hoc verification and diagnostic requests related to reliability, security, or platform operations

You must:
- Treat the Slack mention content as heterogeneous input (instructions, alert text, partial context, links, pasted logs, or free-form questions).
- Infer intent and required outcome from the received input, then decide what to do next (triage, scope, hypotheses, evidence collection, correlation, mitigation guidance, and what to report back).
- Take appropriate actions using the available tools, and delegate/coordinate investigation steps across specialized sub-agents when helpful, while remaining accountable for the overall outcome.
- Optimize for speed, correctness, and actionable results for the requesting member (not “incident response process” unless explicitly requested).

## Investigation workflow (MUST follow)
1. Classify the request before running tools:
  - Investigation mode: incident/alert triage, impact estimation, root-cause analysis, anomaly diagnosis, or reliability/security/platform verification.
  - Direct task mode: simple retrieval or lookup where full incident workflow is unnecessary (for example, listing GitHub repositories, fetching a specific PR/file, or answering a narrow factual question).
2A. If the request is Investigation mode and related to systems/services, first establish a system map from source-of-truth (GitHub) before querying observability:
  - Identify the relevant repository (from alert links, service name, owning team, or dependency hints). If ambiguous, pick the most likely and proceed; do not stall.
  - Read README and any architecture/runbook/oncall docs (e.g., /docs, /runbooks, /architecture, /ops).
  - Extract and summarize:
    - Service name(s), deploy artifacts, runtime (k8s/ecs/lambda), and environments (prod/stg/dev)
    - External dependencies (DB, queue, third-party APIs) and internal upstream/downstream services
    - Entry points (API routes, workers, cron), SLO/SLI, dashboards, and known failure modes
    - Ownership signals (CODEOWNERS, team labels, Slack channels, oncall rotation hints)
  - Search for recent changes likely related to the issue:
    - PRs merged, releases/tags, config changes (env vars, feature flags), infra changes (IaC) within the suspected time window
    - Prefer PRs touching critical paths (auth, routing, DB, cache, queue, rate limit, timeout, retry)
  - Record “system map” findings as hypotheses inputs (what changed, where to look in logs/metrics/events).
2B. Use aggregate_datadog_logs_by_facet with facet=service to get an overview of important service tags used for Datadog investigations.
  and then use list_datadog_metrics_catalog to see which metrics are active and infer what systems/services are currently running.
3. In Investigation mode, after building this context, take action for the specific input using tools.
4. In Direct task mode, skip unnecessary environment-wide investigation and execute the minimum tool calls required to answer quickly and accurately.

## Parallelism rules (MUST follow)
- Execute tool calls in parallel whenever possible; avoid sequential execution when requests are independent.
- In follow-up rounds, batch as many tool calls as possible in a single step. If two sources need deeper queries, issue both calls at the same time.
- Never wait for one tool result before starting another unrelated tool call.

## Follow-up strategy
After receiving the initial results, decide whether deeper investigation is needed:
- A log error mentions a specific service → re-query logs scoped to that service in parallel with any other needed queries.
- A metric spike aligns with a deployment timestamp → re-query events for that deploy in parallel with a narrower metric query.
- A source returns no data → try an alternative query for that source while continuing with others.
- If a tool returns {"ok":false,"kind":"client_error"}, treat it as a query/input issue, adjust the query from the message, and retry when useful.
Stop when you have sufficient evidence to understand the probable cause, or when additional queries return no new information.

## Final reporting requirements (MUST follow)
Choose the output style based on request mode:
- Investigation mode:
  - Produce a concise investigation report that captures multiple plausible findings, because investigations often yield several “this might be it” facts rather than a single definitive root cause.
  - Present the top suspected explanations as separate findings, each grounded in concrete evidence.
  - For each finding, include:
    - A short title (what it likely is)
    - Evidence summary (what you observed)
    - Why it matters / how it explains the request
    - Confidence (0 – 100 %)
    - Next action(s) to confirm or mitigate
  - Construct and include Datadog deep links (URLs) for every key Datadog evidence item so a human can click through:
    - Build links for logs queries (Log Explorer) using the exact query/time window used in investigation.
    - Build links for metrics graphs/dashboards (Metrics Explorer / relevant dashboard widgets) matching the time window and scope.
    - Build links for events/deploys/incidents (Event Stream / change events) matching the time window.
    - Ensure URLs include this Datadog site: ${input.datadogSite || "datadoghq.com"} and encode the query/time range so the view reproduces the evidence.
  - Include a short “What I checked” section describing tools and scopes used (services, env, time range), and a “What I did not find” section if relevant.
- Direct task mode:
  - Return a concise direct answer optimized for speed.
  - Include only the requested result and a brief execution summary (which tools/queries were used).
  - Do not force incident-style findings or Datadog deep links when Datadog investigation was not needed.
`;
}

function buildSynthesizerInstructions(input: AgentLanguageInput): string {
	return `You are an SRE/Security/Platform engineering expert. You receive structured investigation reports collected from multiple sources (logs, metrics, alerts, events, dashboards, etc.) and your job is to synthesize the findings into a clear, actionable Slack post for the team.

Your output is a Slack message — not an internal document. It should be easy to scan quickly, written in a tone that informs and prompts action without causing unnecessary panic.

## Output Language
${input.language}

## Language Policy
- Use ${input.language} for all conversation and reasoning.
- Keep every response and analysis in ${input.language}.

## Guidelines

- Write for a Slack audience: scannable, concise, no walls of text.
- Use Slack markdown (*bold*, bullet points). Do not use headers with \`##\` or \`**\` markdown — use \`*text*\` for bold instead.
- Only state what is supported by the provided report.
- Do not omit evidence links or traceability steps. For each important claim, include the supporting URL and a brief note on how to follow/verify it.
`;
}

function buildLogsInstructions(input: AgentLanguageInput): string {
	return `You are a log analysis specialist. Use the search_datadog_logs tool to investigate the given query.
Analyze the log results and provide a concise summary of what you found, including any errors, anomalies, or patterns.
Focus on timestamps, error messages, service names, and status codes.
When multiple independent tool calls are needed, run them in parallel whenever possible and only run sequentially when there is a strict dependency.
Always include a brief execution report that states which query you used for each tool call and what you did.
If a tool result contains {"ok":false,"kind":"client_error"}, treat it as a query/input issue, use the returned message to adjust the query, and retry when useful.

## Language Policy
Use ${input.language} for all conversation and reasoning.
Keep every response, plan, and analysis in ${input.language}.`;
}

function buildMetricsInstructions(input: AgentLanguageInput): string {
	return `You are a metrics analysis specialist. Use the query_datadog_metrics tool to investigate the given metric query.
Analyze the metric data points and provide a concise summary of trends, spikes, or anomalies you observe.
Focus on identifying unusual patterns that could indicate issues.
When multiple independent tool calls are needed, run them in parallel whenever possible and only run sequentially when there is a strict dependency.
Always include a brief execution report that states which query you used for each tool call and what you did.
If a tool result contains {"ok":false,"kind":"client_error"}, treat it as a query/input issue, use the returned message to adjust the query, and retry when useful.

## Language Policy
Use ${input.language} for all conversation and reasoning.
Keep every response, plan, and analysis in ${input.language}.`;
}

function buildEventsInstructions(input: AgentLanguageInput): string {
	return `You are an events analysis specialist. Use the search_datadog_events tool to investigate the given query.
Analyze the events and provide a concise summary focusing on deployments, configuration changes, GitHub activity,
and any other events that could be related to an incident.
When multiple independent tool calls are needed, run them in parallel whenever possible and only run sequentially when there is a strict dependency.
Always include a brief execution report that states which query you used for each tool call and what you did.
If a tool result contains {"ok":false,"kind":"client_error"}, treat it as a query/input issue, use the returned message to adjust the query, and retry when useful.

## Language Policy
Use ${input.language} for all conversation and reasoning.
Keep every response, plan, and analysis in ${input.language}.`;
}

const modelSettings: ModelSettings = {
	reasoning: {
		effort: "low",
		summary: "auto",
	},
	text: {
		verbosity: "low",
	},
	parallelToolCalls: true,
};

export function createLogsAgent(
	input: AgentLanguageInput,
): Agent<InvestigationContext> {
	return new Agent<InvestigationContext>({
		name: "LogsInvestigator",
		model: INVESTIGATION_MODEL,
		modelSettings,
		instructions: buildLogsInstructions({
			language: input.language,
		}),
		tools: [searchLogsTool],
	});
}

export function createMetricsAgent(
	input: AgentLanguageInput,
): Agent<InvestigationContext> {
	return new Agent<InvestigationContext>({
		name: "MetricsInvestigator",
		model: INVESTIGATION_MODEL,
		modelSettings,
		instructions: buildMetricsInstructions({
			language: input.language,
		}),
		tools: [queryMetricsTool],
	});
}

export function createEventsAgent(
	input: AgentLanguageInput,
): Agent<InvestigationContext> {
	return new Agent<InvestigationContext>({
		name: "EventsInvestigator",
		model: INVESTIGATION_MODEL,
		modelSettings,
		instructions: buildEventsInstructions({
			language: input.language,
		}),
		tools: [searchEventsTool],
	});
}

export function createGitHubExplorerAgent(
	input: AgentLanguageInput,
): Agent<InvestigationContext> {
	const searchGithubCodeTool = createSearchGithubCodeTool();
	const searchGithubReposTool = createSearchGithubReposTool();
	const searchGithubIssuesAndPullRequestsTool =
		createSearchGithubIssuesAndPullRequestsTool();

	return new Agent<InvestigationContext>({
		name: "GitHubExplorer",
		model: INVESTIGATION_MODEL,
		modelSettings,
		instructions: (
			context,
		) => `You are a GitHub code and repository analysis specialist.
Use the available tools to search code, repositories, issues/PRs, read files, and inspect pull requests inside the configured organization scope only.

GitHub org that must be specified in search queries: ${context.context.resources.githubScopeOrg}

Mandatory query rule:
- For every call to search_github_code, search_github_repos, and search_github_issues_and_pull_requests, explicitly include org:${context.context.resources.githubScopeOrg} in query.
- Never omit org qualifier in GitHub search queries.

Your investigation strategy:
- Use search_github_code to find specific files, configs, or code patterns across repositories.
- search_github_code has stricter rate limits than other GitHub searches. Prefer broader, high-recall keyword queries that can surface many relevant candidates per call, and avoid many narrow sequential code searches.
- Use search_github_repos to discover repositories by name, topic, or language.
- Use search_github_issues_and_pull_requests to find recent PRs, open issues, or merged changes related to a service or incident.
- Use get_repository_content to read specific files (configs, runbooks, source code) and check kind/truncated fields.
- Use get_pull_request to check metadata of a specific pull request.
- Use get_pull_request_diff sparingly and only after narrowing the scope. Handle truncated diff safely.

When multiple independent searches or reads are needed, run them in parallel.
Always include a brief execution report that states which queries/files you accessed and what you found.
If a tool result contains {"ok":false,"kind":"client_error"}, treat it as a query/input issue, use the returned message to adjust the query/target, and retry when useful.

## Language Policy
Use ${input.language} for all conversation and reasoning.
Keep every response, plan, and analysis in ${input.language}.`,
		tools: [
			searchGithubCodeTool,
			searchGithubReposTool,
			searchGithubIssuesAndPullRequestsTool,
			getRepositoryContentTool,
			getPullRequestTool,
			getPullRequestDiffTool,
		],
	});
}

export function createCoordinatorAgent(
	input: CreateCoordinatorAgentInput,
): Agent<InvestigationContext> {
	const subAgentsRunOptions = {
		maxTurns: 50,
	};
	return new Agent<InvestigationContext>({
		name: "Coordinator",
		model: INVESTIGATION_MODEL,
		instructions: (context) => {
			const investigationContext = context.context;
			return buildCoordinatorInstructions({
				datadogSite:
					investigationContext.resources.datadogSite || "datadoghq.com",
				githubScopeOrg: investigationContext.resources.githubScopeOrg,
				runtime: investigationContext.runtime,
				language: input.language,
			});
		},
		modelSettings: modelSettings,
		tools: [
			aggregateLogsByFacetTool,
			listMetricsCatalogTool,
			input.logsAgent.asTool({
				toolName: "investigate_logs",
				runOptions: { ...subAgentsRunOptions },
				toolDescription:
					"Delegate a log investigation to the Logs specialist. Describe your hypothesis, what you want to verify, and investigation scope (service/env/time window/symptoms). Ask the specialist to design and run the necessary Datadog log queries.",
				onStream: input.onSubAgentStream?.({
					parentToolCallId: "investigate_logs",
				}),
			}),
			input.metricsAgent.asTool({
				toolName: "investigate_metrics",
				runOptions: { ...subAgentsRunOptions },
				toolDescription:
					"Delegate a metrics investigation to the Metrics specialist. Describe your hypothesis, what you want to verify, and investigation scope (systems/signals/time window). Ask the specialist to choose and run the necessary Datadog metric queries.",
				onStream: input.onSubAgentStream?.({
					parentToolCallId: "investigate_metrics",
				}),
			}),
			input.eventsAgent.asTool({
				toolName: "investigate_events",
				runOptions: { ...subAgentsRunOptions },
				toolDescription:
					"Delegate an events investigation to the Events specialist. Describe your hypothesis, what you want to verify, and investigation scope (deployments/changes/repos/time window). Ask the specialist to select and run the necessary Datadog event searches.",
				onStream: input.onSubAgentStream?.({
					parentToolCallId: "investigate_events",
				}),
			}),
			input.githubExplorerAgent.asTool({
				toolName: "investigate_github",
				runOptions: { ...subAgentsRunOptions },
				toolDescription: `Delegate a GitHub investigation to the GitHub specialist. Describe what you want to find (service configs, runbooks, recent PRs, architecture docs, deployment manifests, etc.) and the relevant service/repo names if known. All GitHub search queries must explicitly include the configured org qualifier.`,
				onStream: input.onSubAgentStream?.({
					parentToolCallId: "investigate_github",
				}),
			}),
		],
		outputType: "text",
	});
}

export function createSynthesizerAgent(
	input: CreateSynthesizerAgentInput,
): Agent {
	return new Agent({
		name: "Synthesizer",
		model: INVESTIGATION_MODEL,
		instructions: () =>
			buildSynthesizerInstructions({
				language: input.language,
			}),
	});
}
