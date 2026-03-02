import { randomUUID } from "node:crypto";
import type { SlackMessageHandlerPort } from "../ports/inbound/slack-message-handler";
import type { SlackThreadReplyPort } from "../ports/outbound/slack-thread-reply";
import type { WorkerJobDispatcherPort } from "../ports/outbound/worker-job-dispatcher";
import type { Logger } from "../shared/observability/logger";
import type {
	AlertInvestigationJob,
	InvestigationJob,
} from "../shared/types/investigation-job";
import type { SlackMessage } from "../shared/types/slack-message";
import { toErrorMessage } from "../shared/utils/to-error-message";

export interface EnqueueSlackEventUseCaseDeps {
	workerJobDispatcher: WorkerJobDispatcherPort;
	slackReplyPort: SlackThreadReplyPort;
	logger: Logger;
	jobMaxRetry: number;
	jobBackoffMs: number;
}

export class EnqueueSlackEventUseCase implements SlackMessageHandlerPort {
	constructor(private readonly deps: EnqueueSlackEventUseCaseDeps) {}

	async handle(message: SlackMessage): Promise<void> {
		const eventStartedAtMs = Date.now();
		const threadTs = message.threadTs ?? message.ts;

		// TODO: Move toward a generic event-to-job translation flow so this use case can accept multiple inbound event kinds.
		const job = buildInvestigationJob({
			message,
			receivedAt: new Date().toISOString(),
		});

		try {
			const dispatchStartedAtMs = Date.now();
			await this.dispatchWithRetry(job);
			const dispatchLatencyMs = Date.now() - dispatchStartedAtMs;

			this.deps.logger.info("Dispatched worker job", {
				slackEventId: message.slackEventId,
				jobId: job.jobId,
				jobType: job.jobType,
				channel: message.channel,
				threadTs,
				ingress_dispatch_latency_ms: dispatchLatencyMs,
				ingress_ack_latency_ms: Date.now() - eventStartedAtMs,
			});
		} catch (error) {
			const errorMessage = toErrorMessage(error);
			this.deps.logger.error("Failed to dispatch worker job", {
				slackEventId: message.slackEventId,
				jobId: job.jobId,
				jobType: job.jobType,
				channel: message.channel,
				threadTs,
				error: errorMessage,
				ingress_ack_latency_ms: Date.now() - eventStartedAtMs,
			});

			await this.deps.slackReplyPort.postThreadReply({
				channel: message.channel,
				threadTs,
				text: `Failed to queue investigation: ${errorMessage}`,
			});
		}
	}

	private async dispatchWithRetry(job: InvestigationJob): Promise<void> {
		let attempt = 0;
		const maxAttempts = this.deps.jobMaxRetry + 1;

		while (attempt < maxAttempts) {
			attempt += 1;
			try {
				await this.deps.workerJobDispatcher.dispatch(job);
				return;
			} catch (error) {
				if (attempt >= maxAttempts) {
					throw error;
				}

				this.deps.logger.warn("Retrying worker dispatch", {
					slackEventId: job.payload.slackEventId,
					jobId: job.jobId,
					jobType: job.jobType,
					attempt,
					remainingAttempts: maxAttempts - attempt,
					error: toErrorMessage(error),
				});

				await sleep(this.deps.jobBackoffMs);
			}
		}
	}
}

interface BuildInvestigationJobInput {
	message: SlackMessage;
	receivedAt: string;
}

function buildInvestigationJob(
	input: BuildInvestigationJobInput,
): InvestigationJob {
	return buildAlertInvestigationJob(input);
}

function buildAlertInvestigationJob(
	input: BuildInvestigationJobInput,
): AlertInvestigationJob {
	return {
		jobId: randomUUID(),
		jobType: "alert_investigation",
		receivedAt: input.receivedAt,
		payload: {
			slackEventId: input.message.slackEventId,
			message: input.message,
		},
		retryCount: 0,
	};
}

function sleep(durationMs: number): Promise<void> {
	return new Promise((resolve) => {
		setTimeout(resolve, durationMs);
	});
}
