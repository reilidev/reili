import type { WorkerJobDispatcherPort } from "../../../ports/outbound/worker-job-dispatcher";
import type { InvestigationJob } from "../../../shared/types/investigation-job";

const DEFAULT_TIMEOUT_MS = 3_000;

export interface HttpWorkerJobDispatcherConfig {
	workerBaseUrl: string;
	workerInternalToken: string;
	timeoutMs?: number;
}

export class HttpWorkerJobDispatcherAdapter implements WorkerJobDispatcherPort {
	private readonly endpoint: string;
	private readonly timeoutMs: number;

	constructor(private readonly config: HttpWorkerJobDispatcherConfig) {
		this.endpoint = buildWorkerEndpoint(config.workerBaseUrl);
		this.timeoutMs = config.timeoutMs ?? DEFAULT_TIMEOUT_MS;
	}

	async dispatch(job: InvestigationJob): Promise<void> {
		const response = await fetch(this.endpoint, {
			method: "POST",
			headers: {
				Authorization: `Bearer ${this.config.workerInternalToken}`,
				"Content-Type": "application/json",
			},
			body: JSON.stringify(job),
			signal: AbortSignal.timeout(this.timeoutMs),
		});

		if (!response.ok) {
			throw new Error(
				`Failed to dispatch investigation job: status=${response.status}`,
			);
		}
	}
}

function buildWorkerEndpoint(workerBaseUrl: string): string {
	const normalizedBaseUrl = workerBaseUrl.replace(/\/+$/, "");
	return `${normalizedBaseUrl}/internal/jobs`;
}
