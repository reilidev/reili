import type { Usage } from "@openai/agents";
import { createEmptyLlmUsageSnapshot } from "../../../capabilities/integration/investigation/build-llm-telemetry";
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
