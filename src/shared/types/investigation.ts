export type InvestigationSource =
	| "datadog_logs"
	| "datadog_metrics"
	| "datadog_events";

export interface InvestigationTask {
	taskId: string;
	source: InvestigationSource;
	priority: number;
	deadlineMs: number;
	payload: Record<string, unknown>;
}

export interface Evidence {
	source: InvestigationSource;
	summary: string;
	raw: Record<string, unknown>;
	observedAt: string;
	confidence: number;
}

export interface InvestigationFailure {
	source: InvestigationSource;
	reason: string;
}

export type InvestigationResult = string;
