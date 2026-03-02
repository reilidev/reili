const DEFAULT_INGRESS_PORT = 3000;
const DEFAULT_WORKER_PORT = 3100;
const DEFAULT_WORKER_CONCURRENCY = 2;
const DEFAULT_DATADOG_SITE = "datadoghq.com";
const DEFAULT_LANGUAGE = "English";
const DEFAULT_JOB_MAX_RETRY = 2;
const DEFAULT_JOB_BACKOFF_MS = 1_000;
const DEFAULT_WORKER_DISPATCH_TIMEOUT_MS = 3_000;

interface SlackAuthConfig {
	slackBotToken: string;
	slackSigningSecret: string;
}

interface InvestigationConfig {
	datadogApiKey: string;
	datadogAppKey: string;
	datadogSite: string;
	openaiApiKey: string;
	language: string;
}

export interface GitHubAppConfig {
	appId: string;
	privateKey: string;
	installationId: number;
	scopeOrg: string;
}

export interface IngressConfig extends SlackAuthConfig {
	port: number;
	workerBaseUrl: string;
	workerInternalToken: string;
	jobMaxRetry: number;
	jobBackoffMs: number;
	workerDispatchTimeoutMs: number;
}

export interface WorkerConfig extends SlackAuthConfig, InvestigationConfig {
	workerInternalPort: number;
	workerInternalToken: string;
	workerConcurrency: number;
	jobMaxRetry: number;
	jobBackoffMs: number;
	github: GitHubAppConfig;
}

export function loadIngressConfig(): IngressConfig {
	return {
		...readSlackAuthConfig(),
		port: readPort({
			name: "PORT",
			value: process.env.PORT,
			defaultValue: DEFAULT_INGRESS_PORT,
		}),
		workerBaseUrl: readRequiredEnv("WORKER_BASE_URL"),
		workerInternalToken: readRequiredEnv("WORKER_INTERNAL_TOKEN"),
		jobMaxRetry: DEFAULT_JOB_MAX_RETRY,
		jobBackoffMs: DEFAULT_JOB_BACKOFF_MS,
		workerDispatchTimeoutMs: DEFAULT_WORKER_DISPATCH_TIMEOUT_MS,
	};
}

export function loadWorkerConfig(): WorkerConfig {
	return {
		...readSlackAuthConfig(),
		...readInvestigationConfig(),
		workerInternalPort: readPort({
			name: "WORKER_INTERNAL_PORT",
			value: process.env.WORKER_INTERNAL_PORT,
			defaultValue: DEFAULT_WORKER_PORT,
		}),
		workerInternalToken: readRequiredEnv("WORKER_INTERNAL_TOKEN"),
		workerConcurrency: DEFAULT_WORKER_CONCURRENCY,
		jobMaxRetry: DEFAULT_JOB_MAX_RETRY,
		jobBackoffMs: DEFAULT_JOB_BACKOFF_MS,
		github: readGitHubAppConfig(),
	};
}

function readSlackAuthConfig(): SlackAuthConfig {
	return {
		slackBotToken: readRequiredEnv("SLACK_BOT_TOKEN"),
		slackSigningSecret: readRequiredEnv("SLACK_SIGNING_SECRET"),
	};
}

function readInvestigationConfig(): InvestigationConfig {
	return {
		datadogApiKey: readRequiredEnv("DATADOG_API_KEY"),
		datadogAppKey: readRequiredEnv("DATADOG_APP_KEY"),
		datadogSite: process.env.DATADOG_SITE ?? DEFAULT_DATADOG_SITE,
		openaiApiKey: readRequiredEnv("OPENAI_API_KEY"),
		language: process.env.LANGUAGE ?? DEFAULT_LANGUAGE,
	};
}

function readGitHubAppConfig(): GitHubAppConfig {
	return {
		appId: readRequiredEnv("GITHUB_APP_ID"),
		privateKey: readRequiredEnv("GITHUB_APP_PRIVATE_KEY").replace(/\\n/g, "\n"),
		installationId: readRequiredPositiveInt(
			"GITHUB_APP_INSTALLATION_ID",
			readRequiredEnv("GITHUB_APP_INSTALLATION_ID"),
		),
		scopeOrg: readRequiredEnv("GITHUB_SEARCH_SCOPE_ORG"),
	};
}

function readRequiredEnv(name: string): string {
	const value = process.env[name];
	if (!value) {
		throw new Error(`Missing required environment variable: ${name}`);
	}

	return value;
}

interface ReadIntegerInput {
	name: string;
	value: string | undefined;
	defaultValue: number;
}

function readRequiredPositiveInt(name: string, value: string): number {
	const parsed = Number(value);
	if (!Number.isInteger(parsed) || parsed <= 0) {
		throw new Error(`Invalid ${name} value: ${value}`);
	}

	return parsed;
}

function readPositiveInt(input: ReadIntegerInput): number {
	if (!input.value) {
		return input.defaultValue;
	}

	const parsed = Number(input.value);
	if (!Number.isInteger(parsed) || parsed <= 0) {
		throw new Error(`Invalid ${input.name} value: ${input.value}`);
	}

	return parsed;
}

function readPort(input: ReadIntegerInput): number {
	const parsed = readPositiveInt(input);
	if (parsed > 65535) {
		throw new Error(`Invalid ${input.name} value: ${input.value}`);
	}

	return parsed;
}
