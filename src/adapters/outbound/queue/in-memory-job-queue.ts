import type {
	CompleteJobInput,
	FailJobInput,
	JobFailResult,
	JobQueuePort,
	QueueJob,
} from "../../../ports/outbound/job-queue";

interface DelayedJob<TJob extends QueueJob> {
	job: TJob;
	availableAtMs: number;
}

interface DeadLetterJob<TJob extends QueueJob> {
	job: TJob;
	reason: string;
	failedAt: string;
}

export class InMemoryJobQueue<TJob extends QueueJob>
	implements JobQueuePort<TJob>
{
	private readonly pendingJobs: TJob[] = [];
	private readonly delayedJobs: DelayedJob<TJob>[] = [];
	private readonly claimedJobs = new Map<string, TJob>();
	private readonly deadLetterJobs: DeadLetterJob<TJob>[] = [];

	async enqueue(job: TJob): Promise<void> {
		this.pendingJobs.push(job);
	}

	async claim(): Promise<TJob | undefined> {
		this.moveReadyDelayedJobsToPending();
		const nextJob = this.pendingJobs.shift();
		if (!nextJob) {
			return undefined;
		}

		this.claimedJobs.set(nextJob.jobId, nextJob);
		return nextJob;
	}

	async complete(input: CompleteJobInput): Promise<void> {
		this.claimedJobs.delete(input.jobId);
	}

	async fail(input: FailJobInput): Promise<JobFailResult<TJob>> {
		const claimedJob = this.claimedJobs.get(input.jobId);

		if (!claimedJob) {
			throw new Error(`Claimed job not found: jobId=${input.jobId}`);
		}

		this.claimedJobs.delete(input.jobId);

		if (claimedJob.retryCount >= input.maxRetry) {
			this.deadLetterJobs.push({
				job: claimedJob,
				reason: input.reason,
				failedAt: new Date().toISOString(),
			});

			return {
				status: "dead_letter",
				job: claimedJob,
			};
		}

		const retriedJob: TJob = {
			...claimedJob,
			retryCount: claimedJob.retryCount + 1,
		};
		this.delayedJobs.push({
			job: retriedJob,
			availableAtMs: Date.now() + computeBackoffMs(input, retriedJob),
		});

		return {
			status: "requeued",
			job: retriedJob,
		};
	}

	async getDepth(): Promise<number> {
		return this.pendingJobs.length + this.delayedJobs.length;
	}

	private moveReadyDelayedJobsToPending(): void {
		if (this.delayedJobs.length === 0) {
			return;
		}

		const nowMs = Date.now();
		const remainingDelayedJobs: DelayedJob<TJob>[] = [];

		for (const delayedJob of this.delayedJobs) {
			if (delayedJob.availableAtMs <= nowMs) {
				this.pendingJobs.push(delayedJob.job);
				continue;
			}

			remainingDelayedJobs.push(delayedJob);
		}

		this.delayedJobs.length = 0;
		this.delayedJobs.push(...remainingDelayedJobs);
	}
}

function computeBackoffMs(input: FailJobInput, job: QueueJob): number {
	const multiplier = 2 ** Math.max(0, job.retryCount - 1);
	return input.backoffMs * multiplier;
}
