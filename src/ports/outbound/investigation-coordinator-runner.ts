import type { AlertContext } from "../../shared/types/alert-context";
import type { InvestigationResult } from "../../shared/types/investigation";
import type { LlmUsageSnapshot } from "../../shared/types/investigation-llm-telemetry";
import type { InvestigationContext } from "./investigation-context";
import type { InvestigationProgressEventCallback } from "./investigation-progress-event";

const COORDINATOR_RUN_FAILED_ERROR_CODE = "COORDINATOR_RUN_FAILED";

export interface CoordinatorRunReport {
	resultText: InvestigationResult;
	usage: LlmUsageSnapshot;
}

export interface RunCoordinatorInput {
	alertContext: AlertContext;
	context: InvestigationContext;
	signal: AbortSignal;
	onProgressEvent: InvestigationProgressEventCallback;
}

interface CoordinatorRunFailedErrorInput {
	usage: LlmUsageSnapshot;
	cause: unknown;
}

export class CoordinatorRunFailedError extends Error {
	readonly code = COORDINATOR_RUN_FAILED_ERROR_CODE;
	readonly usage: LlmUsageSnapshot;
	override readonly cause: unknown;

	constructor(input: CoordinatorRunFailedErrorInput) {
		super("Coordinator agent run failed");
		this.name = "CoordinatorRunFailedError";
		this.usage = input.usage;
		this.cause = input.cause;
	}
}

export interface InvestigationCoordinatorRunnerPort {
	run(input: RunCoordinatorInput): Promise<CoordinatorRunReport>;
}

export function isCoordinatorRunFailedError(
	error: unknown,
): error is CoordinatorRunFailedError {
	const candidate = Object(error) as Partial<CoordinatorRunFailedError>;
	return candidate.code === COORDINATOR_RUN_FAILED_ERROR_CODE;
}
