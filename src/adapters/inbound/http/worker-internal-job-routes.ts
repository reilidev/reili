import { Router } from "express";
import type { InvestigationJobQueuePort } from "../../../ports/outbound/investigation-job-queue";
import type { Logger } from "../../../shared/observability/logger";
import { investigationJobSchema } from "../../../shared/validation/investigation-job-schema";
import { createWorkerInternalAuthMiddleware } from "./worker-internal-auth-middleware";

export const workerInternalJobPath = "/internal/jobs";

export interface WorkerInternalJobRoutesDeps {
	jobQueue: InvestigationJobQueuePort;
	workerInternalToken: string;
	logger: Logger;
}

export function createWorkerInternalJobRouter(
	deps: WorkerInternalJobRoutesDeps,
): Router {
	const router = Router();

	router.post(
		workerInternalJobPath,
		createWorkerInternalAuthMiddleware({
			workerInternalToken: deps.workerInternalToken,
		}),
		async (request, response) => {
			const parsedPayload = investigationJobSchema.safeParse(request.body);
			if (!parsedPayload.success) {
				response.status(400).send("Invalid payload");
				return;
			}

			await deps.jobQueue.enqueue(parsedPayload.data);
			const queueDepth = await deps.jobQueue.getDepth();

			deps.logger.info("Queued investigation job", {
				jobId: parsedPayload.data.jobId,
				jobType: parsedPayload.data.jobType,
				slackEventId: parsedPayload.data.payload.slackEventId,
				channel: parsedPayload.data.payload.message.channel,
				threadTs:
					parsedPayload.data.payload.message.threadTs ??
					parsedPayload.data.payload.message.ts,
				worker_queue_depth: queueDepth,
			});

			response.status(202).send("Accepted");
		},
	);

	return router;
}
