import type {
	BuildInvestigationLlmTelemetryInput,
	InvestigationLlmTelemetry,
	LlmUsageSnapshot,
} from "../../../shared/types/investigation-llm-telemetry";

const EMPTY_LLM_USAGE_SNAPSHOT: LlmUsageSnapshot = {
	requests: 0,
	inputTokens: 0,
	outputTokens: 0,
	totalTokens: 0,
};

export function createEmptyLlmUsageSnapshot(): LlmUsageSnapshot {
	return {
		...EMPTY_LLM_USAGE_SNAPSHOT,
	};
}

export function buildInvestigationLlmTelemetry(
	input: BuildInvestigationLlmTelemetryInput,
): InvestigationLlmTelemetry {
	return {
		coordinator: input.coordinatorUsage,
		synthesizer: input.synthesizerUsage,
		total: addLlmUsageSnapshots(input.coordinatorUsage, input.synthesizerUsage),
	};
}

function addLlmUsageSnapshots(
	left: LlmUsageSnapshot,
	right: LlmUsageSnapshot,
): LlmUsageSnapshot {
	return {
		requests: left.requests + right.requests,
		inputTokens: left.inputTokens + right.inputTokens,
		outputTokens: left.outputTokens + right.outputTokens,
		totalTokens: left.totalTokens + right.totalTokens,
	};
}
