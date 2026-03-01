export interface DatadogLogSearchResult {
	id: string;
	timestamp: string;
	service?: string;
	status?: string;
	message?: string;
	attributes?: Record<string, unknown>;
}

export interface DatadogLogSearchParams {
	query: string;
	from: string;
	to: string;
	limit: number;
}

export interface DatadogLogSearchPort {
	searchLogs(params: DatadogLogSearchParams): Promise<DatadogLogSearchResult[]>;
}
