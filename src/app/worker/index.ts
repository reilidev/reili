import express, { type ErrorRequestHandler } from "express";
import { z } from "zod";
import {
	createWorkerInternalJobRouter,
	workerInternalJobPath,
} from "../../adapters/inbound/http/worker-internal-job-routes";
import { InMemoryJobQueue } from "../../adapters/outbound/queue/in-memory-job-queue";
import { ProcessAlertInvestigationJobUseCase } from "../../application/investigation/process-alert-investigation-job";
import { StartInvestigationWorkerRunnerUseCase } from "../../application/start-investigation-worker-runner";
import type { InvestigationJobQueuePort } from "../../ports/outbound/investigation-job-queue";
import { createLogger } from "../../shared/observability/logger";
import type { InvestigationJob } from "../../shared/types/investigation-job";
import { toErrorMessage } from "../../shared/utils/to-error-message";
import { buildWorkerRuntimeDeps } from "../bootstrap/build-shared-deps";
import { loadWorkerConfig } from "../config/env";

const jsonBodyParseErrorSchema = z.object({
	type: z.literal("entity.parse.failed"),
});

async function main(): Promise<void> {
	const config = loadWorkerConfig();
	const sharedDeps = buildWorkerRuntimeDeps({
		slack: {
			botToken: config.slackBotToken,
			signingSecret: config.slackSigningSecret,
		},
		datadog: {
			apiKey: config.datadogApiKey,
			appKey: config.datadogAppKey,
			site: config.datadogSite,
		},
		openai: {
			apiKey: config.openaiApiKey,
		},
		synthesizer: {
			outputLanguage: config.language,
		},
		conversation: {
			language: config.language,
		},
		github: config.github,
	});
	const jobQueue: InvestigationJobQueuePort =
		new InMemoryJobQueue<InvestigationJob>();

	const processAlertInvestigationJobUseCase =
		new ProcessAlertInvestigationJobUseCase({
			slackReplyPort: sharedDeps.slackReplyPort,
			slackProgressStreamPort: sharedDeps.slackProgressStreamPort,
			slackThreadHistoryPort: sharedDeps.slackThreadHistoryPort,
			investigationResources: sharedDeps.investigationResources,
			coordinatorRunner: sharedDeps.coordinatorRunner,
			synthesizerRunner: sharedDeps.synthesizerRunner,
			logger: sharedDeps.logger,
		});

	const startInvestigationWorkerRunnerUseCase =
		new StartInvestigationWorkerRunnerUseCase({
			jobQueue,
			alertInvestigationProcessor: processAlertInvestigationJobUseCase,
			slackReplyPort: sharedDeps.slackReplyPort,
			logger: sharedDeps.logger,
			workerConcurrency: config.workerConcurrency,
			jobMaxRetry: config.jobMaxRetry,
			jobBackoffMs: config.jobBackoffMs,
		});
	startInvestigationWorkerRunnerUseCase.start();

	const app = express();
	app.use(express.json());
	app.use(
		createWorkerInternalJobRouter({
			jobQueue,
			workerInternalToken: config.workerInternalToken,
			logger: sharedDeps.logger,
		}),
	);
	app.use((_request, response) => {
		response.status(404).send("Not Found");
	});

	const errorHandler: ErrorRequestHandler = (
		error,
		_request,
		response,
		_next,
	) => {
		const normalizedError: unknown = error;
		if (isJsonBodyParseError(normalizedError)) {
			response.status(400).send("Invalid payload");
			return;
		}

		sharedDeps.logger.error("Failed to handle worker internal request", {
			error: toErrorMessage(normalizedError),
		});
		response.status(500).send("Internal Server Error");
	};
	app.use(errorHandler);

	app.listen(config.workerInternalPort, () => {
		sharedDeps.logger.info("Worker app is running", {
			workerInternalPort: config.workerInternalPort,
			workerConcurrency: config.workerConcurrency,
			internalApiPath: workerInternalJobPath,
		});
	});
}

function isJsonBodyParseError(error: unknown): boolean {
	return jsonBodyParseErrorSchema.safeParse(error).success;
}

main().catch((error) => {
	const logger = createLogger();
	logger.error("Worker app failed to start", {
		error: toErrorMessage(error),
	});
	process.exit(1);
});
