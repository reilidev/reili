import type { Usage } from "@openai/agents";
import { createEmptyLlmUsageSnapshot } from "../../../application/investigation/services/build-llm-telemetry";
import type { LlmUsageSnapshot } from "../../../shared/types/investigation-llm-telemetry";

export function mapOpenAiUsageToLlmUsageSnapshot(
	usage?: Usage,
): LlmUsageSnapshot {
	if (!usage) {
		return createEmptyLlmUsageSnapshot();
	}

	return {
		requests: usage.requests,
		inputTokens: usage.inputTokens,
		outputTokens: usage.outputTokens,
		totalTokens: usage.totalTokens,
	};
}
