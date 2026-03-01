import type { InvestigationJob } from "../../shared/types/investigation-job";

export interface WorkerJobDispatcherPort {
	dispatch(job: InvestigationJob): Promise<void>;
}
