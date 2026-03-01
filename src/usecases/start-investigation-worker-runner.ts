import type { InvestigationJobQueuePort } from "../ports/outbound/investigation-job-queue";
import type { SlackThreadReplyPort } from "../ports/outbound/slack-thread-reply";
import type { Logger } from "../shared/observability/logger";
import type { InvestigationJob } from "../shared/types/investigation-job";
import { toErrorMessage } from "../shared/utils/to-error-message";
import type { ProcessAlertInvestigationJobUseCase } from "./investigation/process-alert-investigation-job";

const IDLE_WAIT_MS = 150;

export interface StartInvestigationWorkerRunnerUseCaseDeps {
	jobQueue: InvestigationJobQueuePort;
	alertInvestigationProcessor: ProcessAlertInvestigationJobUseCase;
	slackReplyPort: SlackThreadReplyPort;
	logger: Logger;
	workerConcurrency: number;
	jobMaxRetry: number;
	jobBackoffMs: number;
}

export class StartInvestigationWorkerRunnerUseCase {
	private isRunning = false;

	constructor(
		private readonly deps: StartInvestigationWorkerRunnerUseCaseDeps,
	) {}

	start(): void {
		if (this.isRunning) {
			return;
		}

		this.isRunning = true;
		for (
			let workerIndex = 0;
			workerIndex < this.deps.workerConcurrency;
			workerIndex += 1
		) {
			this.runInvestigationWorkerLoop(workerIndex);
		}
	}

	private async runInvestigationWorkerLoop(workerIndex: number): Promise<void> {
		while (this.isRunning) {
			const job = await this.deps.jobQueue.claim();
			if (!job) {
				await sleep(IDLE_WAIT_MS);
				continue;
			}

			const startedAtMs = Date.now();

			try {
				await this.processJob(job);
				await this.deps.jobQueue.complete({ jobId: job.jobId });
				const queueDepth = await this.deps.jobQueue.getDepth();

				this.deps.logger.info("Completed worker job", {
					workerIndex,
					jobType: job.jobType,
					slackEventId: job.payload.slackEventId,
					jobId: job.jobId,
					channel: job.payload.message.channel,
					threadTs: job.payload.message.threadTs ?? job.payload.message.ts,
					attempt: job.retryCount + 1,
					worker_job_duration_ms: Date.now() - startedAtMs,
					worker_queue_depth: queueDepth,
				});
			} catch (error) {
				const errorMessage = toErrorMessage(error);
				const failResult = await this.deps.jobQueue.fail({
					jobId: job.jobId,
					reason: errorMessage,
					maxRetry: this.deps.jobMaxRetry,
					backoffMs: this.deps.jobBackoffMs,
				});
				const queueDepth = await this.deps.jobQueue.getDepth();

				this.deps.logger.error("Failed worker job", {
					workerIndex,
					jobType: job.jobType,
					slackEventId: job.payload.slackEventId,
					jobId: job.jobId,
					channel: job.payload.message.channel,
					threadTs: job.payload.message.threadTs ?? job.payload.message.ts,
					attempt: job.retryCount + 1,
					worker_job_duration_ms: Date.now() - startedAtMs,
					worker_queue_depth: queueDepth,
					worker_job_failure_total: 1,
					status: failResult.status,
					error: errorMessage,
				});

				if (failResult.status === "dead_letter") {
					try {
						await this.postDeadLetterFailureMessage(
							failResult.job,
							errorMessage,
						);
					} catch (deadLetterError) {
						this.deps.logger.error("Failed dead-letter notification", {
							jobType: failResult.job.jobType,
							slackEventId: failResult.job.payload.slackEventId,
							jobId: failResult.job.jobId,
							channel: failResult.job.payload.message.channel,
							threadTs:
								failResult.job.payload.message.threadTs ??
								failResult.job.payload.message.ts,
							error: toErrorMessage(deadLetterError),
						});
					}
				}
			}
		}
	}

	private async processJob(job: InvestigationJob): Promise<void> {
		await this.deps.alertInvestigationProcessor.handle(job);
	}

	private async postDeadLetterFailureMessage(
		job: InvestigationJob,
		errorMessage: string,
	): Promise<void> {
		await this.deps.slackReplyPort.postThreadReply({
			channel: job.payload.message.channel,
			threadTs: job.payload.message.threadTs ?? job.payload.message.ts,
			text: `Investigation failed after retries: ${errorMessage}`,
		});
	}
}

function sleep(durationMs: number): Promise<void> {
	return new Promise((resolve) => {
		setTimeout(resolve, durationMs);
	});
}
