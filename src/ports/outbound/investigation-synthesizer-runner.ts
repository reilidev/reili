import type { AlertContext } from "../../shared/types/alert-context";
import type { InvestigationResult } from "../../shared/types/investigation";
import type { LlmUsageSnapshot } from "../../shared/types/investigation-llm-telemetry";
import type { InvestigationProgressEventCallback } from "./investigation-progress-event";

const SYNTHESIZER_RUN_FAILED_ERROR_CODE = "SYNTHESIZER_RUN_FAILED";

export interface SynthesizerRunReport {
	reportText: string;
	usage: LlmUsageSnapshot;
}

export interface RunSynthesizerInput {
	result: InvestigationResult;
	alertContext: AlertContext;
	onProgressEvent: InvestigationProgressEventCallback;
}

interface SynthesizerRunFailedErrorInput {
	usage: LlmUsageSnapshot;
	cause: unknown;
}

export class SynthesizerRunFailedError extends Error {
	readonly code = SYNTHESIZER_RUN_FAILED_ERROR_CODE;
	readonly usage: LlmUsageSnapshot;
	override readonly cause: unknown;

	constructor(input: SynthesizerRunFailedErrorInput) {
		super("Synthesizer agent run failed");
		this.name = "SynthesizerRunFailedError";
		this.usage = input.usage;
		this.cause = input.cause;
	}
}

export interface InvestigationSynthesizerRunnerPort {
	run(input: RunSynthesizerInput): Promise<SynthesizerRunReport>;
}

export function isSynthesizerRunFailedError(
	error: unknown,
): error is SynthesizerRunFailedError {
	const candidate = Object(error) as Partial<SynthesizerRunFailedError>;
	return candidate.code === SYNTHESIZER_RUN_FAILED_ERROR_CODE;
}
