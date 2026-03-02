import { afterEach, describe, expect, it } from "vitest";
import { loadIngressConfig, loadWorkerConfig } from "./env";

const ORIGINAL_ENV = { ...process.env };

interface EnvOverrides {
	[key: string]: string;
}

function setTestEnv(overrides: EnvOverrides): void {
	process.env = {
		...ORIGINAL_ENV,
		SLACK_BOT_TOKEN: "xoxb-test",
		SLACK_SIGNING_SECRET: "signing-secret",
		WORKER_BASE_URL: "http://localhost:3100",
		WORKER_INTERNAL_TOKEN: "internal-token",
		DATADOG_API_KEY: "dd-api-key",
		DATADOG_APP_KEY: "dd-app-key",
		OPENAI_API_KEY: "openai-api-key",
		GITHUB_APP_ID: "12345",
		GITHUB_APP_PRIVATE_KEY:
			"-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----",
		GITHUB_APP_INSTALLATION_ID: "123456",
		GITHUB_SEARCH_SCOPE_ORG: "example-org",
		...overrides,
	};
}

afterEach(() => {
	process.env = { ...ORIGINAL_ENV };
});

describe("env config", () => {
	it("uses fixed retry settings for ingress even when env vars are set", () => {
		setTestEnv({
			JOB_MAX_RETRY: "99",
			JOB_BACKOFF_MS: "9999",
			WORKER_DISPATCH_TIMEOUT_MS: "9999",
		});

		const config = loadIngressConfig();

		expect(config.jobMaxRetry).toBe(2);
		expect(config.jobBackoffMs).toBe(1000);
		expect(config.workerDispatchTimeoutMs).toBe(3000);
	});

	it("uses fixed worker settings even when env vars are set", () => {
		setTestEnv({
			WORKER_CONCURRENCY: "9",
			JOB_MAX_RETRY: "99",
			JOB_BACKOFF_MS: "9999",
		});

		const config = loadWorkerConfig();

		expect(config.workerConcurrency).toBe(2);
		expect(config.jobMaxRetry).toBe(2);
		expect(config.jobBackoffMs).toBe(1000);
	});
});
