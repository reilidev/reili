import { isCoordinatorRunFailedError } from "../../ports/outbound/investigation-coordinator-runner";
import { isSynthesizerRunFailedError } from "../../ports/outbound/investigation-synthesizer-runner";
import type {
	InvestigationLlmTelemetry,
	LlmUsageSnapshot,
} from "../../shared/types/investigation-llm-telemetry";
import { createEmptyLlmUsageSnapshot } from "./services/build-llm-telemetry";

const INVESTIGATION_EXECUTION_FAILED_ERROR_CODE =
	"INVESTIGATION_EXECUTION_FAILED";

export interface InvestigationExecutionFailedErrorInput {
	cause: unknown;
	llmTelemetry: InvestigationLlmTelemetry;
}

export class InvestigationExecutionFailedError extends Error {
	readonly code = INVESTIGATION_EXECUTION_FAILED_ERROR_CODE;
	override readonly cause: unknown;
	readonly coordinatorUsage: LlmUsageSnapshot;
	readonly synthesizerUsage: LlmUsageSnapshot;

	constructor(input: InvestigationExecutionFailedErrorInput) {
		super("Investigation execution failed");
		this.name = "InvestigationExecutionFailedError";
		this.cause = input.cause;
		this.coordinatorUsage = input.llmTelemetry.coordinator;
		this.synthesizerUsage = input.llmTelemetry.synthesizer;
	}
}

interface ResolvedInvestigationFailureError {
	error: unknown;
	coordinatorUsage: LlmUsageSnapshot;
	synthesizerUsage: LlmUsageSnapshot;
}

export function createInvestigationExecutionFailedError(
	input: InvestigationExecutionFailedErrorInput,
): InvestigationExecutionFailedError {
	return new InvestigationExecutionFailedError(input);
}

export function resolveInvestigationFailureError(
	error: unknown,
): ResolvedInvestigationFailureError {
	if (isInvestigationExecutionFailedError(error)) {
		return {
			error: error.cause,
			coordinatorUsage: error.coordinatorUsage,
			synthesizerUsage: error.synthesizerUsage,
		};
	}

	if (isCoordinatorRunFailedError(error)) {
		return {
			error: error.cause,
			coordinatorUsage: error.usage,
			synthesizerUsage: createEmptyLlmUsageSnapshot(),
		};
	}

	if (isSynthesizerRunFailedError(error)) {
		return {
			error: error.cause,
			coordinatorUsage: createEmptyLlmUsageSnapshot(),
			synthesizerUsage: error.usage,
		};
	}

	return {
		error,
		coordinatorUsage: createEmptyLlmUsageSnapshot(),
		synthesizerUsage: createEmptyLlmUsageSnapshot(),
	};
}

function isInvestigationExecutionFailedError(
	error: unknown,
): error is InvestigationExecutionFailedError {
	const candidate = Object(error) as Partial<InvestigationExecutionFailedError>;
	return candidate.code === INVESTIGATION_EXECUTION_FAILED_ERROR_CODE;
}
