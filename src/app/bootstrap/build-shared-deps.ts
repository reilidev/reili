import { setDefaultOpenAIKey } from "@openai/agents";
import { App, LogLevel } from "@slack/bolt";
import {
	createCoordinatorAgent,
	createEventsAgent,
	createGitHubExplorerAgent,
	createLogsAgent,
	createMetricsAgent,
	createSynthesizerAgent,
} from "../../adapters/outbound/agents/investigation-agents";
import { OpenAiInvestigationCoordinatorRunner } from "../../adapters/outbound/agents/openai-investigation-coordinator-runner";
import { OpenAiInvestigationSynthesizerRunner } from "../../adapters/outbound/agents/openai-investigation-synthesizer-runner";
import type { OpenAiSubAgentStreamCallbackFactory } from "../../adapters/outbound/agents/openai-subagent-stream-callback";
import { createDatadogClientConfig } from "../../adapters/outbound/datadog/create-datadog-client-config";
import { DatadogEventSearchAdapter } from "../../adapters/outbound/datadog/event-search";
import { DatadogLogAggregateAdapter } from "../../adapters/outbound/datadog/log-aggregate";
import { DatadogLogSearchAdapter } from "../../adapters/outbound/datadog/log-search";
import { DatadogMetricCatalogAdapter } from "../../adapters/outbound/datadog/metric-catalog";
import { DatadogMetricQueryAdapter } from "../../adapters/outbound/datadog/metric-query";
import { GitHubSearchAdapter } from "../../adapters/outbound/github/github-search";
import { BoltSlackThreadReplyAdapter } from "../../adapters/outbound/slack/bolt-slack-thread-reply-adapter";
import { BoltSlackProgressStreamAdapter } from "../../adapters/outbound/slack/progress-stream-adapter";
import { SlackThreadHistoryAdapter } from "../../adapters/outbound/slack/slack-thread-history-adapter";
import type { InvestigationResources } from "../../ports/outbound/investigation-context";
import type { InvestigationCoordinatorRunnerPort } from "../../ports/outbound/investigation-coordinator-runner";
import type { InvestigationSynthesizerRunnerPort } from "../../ports/outbound/investigation-synthesizer-runner";
import type { SlackProgressStreamPort } from "../../ports/outbound/slack-progress-stream";
import type { SlackThreadHistoryPort } from "../../ports/outbound/slack-thread-history";
import type { SlackThreadReplyPort } from "../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../shared/observability/logger";
import { createLogger } from "../../shared/observability/logger";
import type { DatadogApiRetryConfig } from "../../shared/types/datadog-api-retry";
import type { GitHubAppConfig } from "../config/env";

export interface SlackAppConfig {
	botToken: string;
	signingSecret: string;
}

export interface DatadogAppConfig {
	apiKey: string;
	appKey: string;
	site: string;
}

export interface OpenAIAppConfig {
	apiKey: string;
}

export interface SynthesizerAppConfig {
	outputLanguage: string;
}

export interface ConversationAppConfig {
	language: string;
}

export interface WorkerRuntimeDepsConfig {
	slack: SlackAppConfig;
	datadog: DatadogAppConfig;
	openai: OpenAIAppConfig;
	synthesizer: SynthesizerAppConfig;
	conversation: ConversationAppConfig;
	github: GitHubAppConfig;
}

const DATADOG_API_RETRY_CONFIG: DatadogApiRetryConfig = {
	enabled: true,
	maxRetries: 3,
	backoffBaseSeconds: 2,
	backoffMultiplier: 2,
};

export interface WorkerRuntimeDeps {
	logger: Logger;
	slackReplyPort: SlackThreadReplyPort;
	slackProgressStreamPort: SlackProgressStreamPort;
	slackThreadHistoryPort: SlackThreadHistoryPort;
	investigationResources: InvestigationResources;
	coordinatorRunner: InvestigationCoordinatorRunnerPort;
	synthesizerRunner: InvestigationSynthesizerRunnerPort;
}

export function createSlackBoltApp(config: SlackAppConfig): App {
	return new App({
		token: config.botToken,
		signingSecret: config.signingSecret,
		endpoints: "/slack/events",
		logLevel: LogLevel.INFO,
	});
}

export function buildWorkerRuntimeDeps(
	config: WorkerRuntimeDepsConfig,
	logger: Logger = createLogger(),
): WorkerRuntimeDeps {
	setDefaultOpenAIKey(config.openai.apiKey);

	const slackApp = createSlackBoltApp(config.slack);
	const slackReplyPort = new BoltSlackThreadReplyAdapter(slackApp.client);
	const slackProgressStreamPort = new BoltSlackProgressStreamAdapter(
		slackApp.client,
	);
	const slackThreadHistoryPort = new SlackThreadHistoryAdapter(slackApp.client);
	const datadogClientConfig = createDatadogClientConfig({
		apiKey: config.datadog.apiKey,
		appKey: config.datadog.appKey,
		site: config.datadog.site,
		retry: DATADOG_API_RETRY_CONFIG,
	});
	const logAggregatePort = new DatadogLogAggregateAdapter(datadogClientConfig);
	const logSearchPort = new DatadogLogSearchAdapter(datadogClientConfig);
	const metricQueryPort = new DatadogMetricQueryAdapter(datadogClientConfig);
	const metricCatalogPort = new DatadogMetricCatalogAdapter(
		datadogClientConfig,
	);
	const eventSearchPort = new DatadogEventSearchAdapter(datadogClientConfig);
	const githubSearchPort = new GitHubSearchAdapter({
		appId: config.github.appId,
		privateKey: config.github.privateKey,
		installationId: config.github.installationId,
		scopeOrg: config.github.scopeOrg,
	});
	const investigationResources: InvestigationResources = {
		logAggregatePort,
		logSearchPort,
		metricCatalogPort,
		metricQueryPort,
		eventSearchPort,
		datadogSite: config.datadog.site,
		githubScopeOrg: config.github.scopeOrg,
		githubSearchPort,
	};
	const coordinatorAgentFactory = (input: {
		onSubAgentStream?: OpenAiSubAgentStreamCallbackFactory;
	}) => {
		const logsAgent = createLogsAgent({
			language: config.conversation.language,
		});
		const metricsAgent = createMetricsAgent({
			language: config.conversation.language,
		});
		const eventsAgent = createEventsAgent({
			language: config.conversation.language,
		});
		const githubExplorerAgent = createGitHubExplorerAgent({
			language: config.conversation.language,
		});
		return createCoordinatorAgent({
			logsAgent,
			metricsAgent,
			eventsAgent,
			githubExplorerAgent,
			onSubAgentStream: input.onSubAgentStream,
			language: config.conversation.language,
		});
	};
	const synthesizerAgentFactory = () =>
		createSynthesizerAgent({
			language: config.synthesizer.outputLanguage,
		});
	const coordinatorRunner = new OpenAiInvestigationCoordinatorRunner({
		createCoordinatorAgent: coordinatorAgentFactory,
		logger,
	});
	const synthesizerRunner = new OpenAiInvestigationSynthesizerRunner({
		createSynthesizerAgent: synthesizerAgentFactory,
	});

	return {
		logger,
		slackReplyPort,
		slackProgressStreamPort,
		slackThreadHistoryPort,
		investigationResources,
		coordinatorRunner,
		synthesizerRunner,
	};
}
