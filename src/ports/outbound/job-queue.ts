export interface QueueJob {
	jobId: string;
	retryCount: number;
}

export interface CompleteJobInput {
	jobId: string;
}

export interface FailJobInput {
	jobId: string;
	reason: string;
	maxRetry: number;
	backoffMs: number;
}

export type JobFailStatus = "requeued" | "dead_letter";

export interface JobFailResult<TJob extends QueueJob> {
	status: JobFailStatus;
	job: TJob;
}

export interface JobQueuePort<TJob extends QueueJob> {
	enqueue(job: TJob): Promise<void>;
	claim(): Promise<TJob | undefined>;
	complete(input: CompleteJobInput): Promise<void>;
	fail(input: FailJobInput): Promise<JobFailResult<TJob>>;
	getDepth(): Promise<number>;
}
