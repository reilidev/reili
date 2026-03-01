export interface LlmUsageSnapshot {
	requests: number;
	inputTokens: number;
	outputTokens: number;
	totalTokens: number;
}

export interface InvestigationLlmTelemetry {
	coordinator: LlmUsageSnapshot;
	synthesizer: LlmUsageSnapshot;
	total: LlmUsageSnapshot;
}

export interface BuildInvestigationLlmTelemetryInput {
	coordinatorUsage: LlmUsageSnapshot;
	synthesizerUsage: LlmUsageSnapshot;
}
