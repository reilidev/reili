import type { InvestigationJob } from "../../shared/types/investigation-job";
import type { JobQueuePort } from "./job-queue";

export type InvestigationJobQueuePort = JobQueuePort<InvestigationJob>;
