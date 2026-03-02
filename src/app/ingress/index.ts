import { registerSlackEventHandlers } from "../../adapters/inbound/slack/register-slack-event-handlers";
import { BoltSlackThreadReplyAdapter } from "../../adapters/outbound/slack/bolt-slack-thread-reply-adapter";
import { HttpWorkerJobDispatcherAdapter } from "../../adapters/outbound/worker/http-worker-job-dispatcher";
import { EnqueueSlackEventUseCase } from "../../application/enqueue-slack-event";
import { createLogger } from "../../shared/observability/logger";
import { toErrorMessage } from "../../shared/utils/to-error-message";
import { createSlackBoltApp } from "../bootstrap/build-shared-deps";
import { loadIngressConfig } from "../config/env";

async function main(): Promise<void> {
	const config = loadIngressConfig();
	const logger = createLogger();

	const app = createSlackBoltApp({
		botToken: config.slackBotToken,
		signingSecret: config.slackSigningSecret,
	});
	const slackReplyAdapter = new BoltSlackThreadReplyAdapter(app.client);
	const workerDispatcher = new HttpWorkerJobDispatcherAdapter({
		workerBaseUrl: config.workerBaseUrl,
		workerInternalToken: config.workerInternalToken,
		timeoutMs: config.workerDispatchTimeoutMs,
	});
	const enqueueSlackEventUseCase = new EnqueueSlackEventUseCase({
		workerJobDispatcher: workerDispatcher,
		slackReplyPort: slackReplyAdapter,
		logger,
		jobMaxRetry: config.jobMaxRetry,
		jobBackoffMs: config.jobBackoffMs,
	});

	registerSlackEventHandlers(app, enqueueSlackEventUseCase, logger);
	await app.start(config.port);

	logger.info("Ingress app is running", {
		port: config.port,
		eventsEndpoint: "/slack/events",
		workerBaseUrl: config.workerBaseUrl,
	});
}

main().catch((error) => {
	const logger = createLogger();
	logger.error("Ingress app failed to start", {
		error: toErrorMessage(error),
	});
	process.exit(1);
});
