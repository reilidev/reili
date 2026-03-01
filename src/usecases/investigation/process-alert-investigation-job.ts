import type { AlertInvestigationJob } from "../../shared/types/investigation-job";
import {
	executeInvestigationJob,
	type InvestigationExecutionDeps,
} from "./execute-investigation-job";

export type ProcessAlertInvestigationJobUseCaseDeps =
	InvestigationExecutionDeps;

export class ProcessAlertInvestigationJobUseCase {
	constructor(private readonly deps: ProcessAlertInvestigationJobUseCaseDeps) {}

	async handle(job: AlertInvestigationJob): Promise<void> {
		await executeInvestigationJob({
			jobType: job.jobType,
			jobId: job.jobId,
			retryCount: job.retryCount,
			payload: job.payload,
			deps: this.deps,
		});
	}
}
