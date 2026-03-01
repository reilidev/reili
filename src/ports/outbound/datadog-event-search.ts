export interface DatadogEventSearchResult {
	id: string;
	timestamp: string;
	source?: string;
	status?: string;
	title?: string;
	message?: string;
	tags?: string[];
}

export interface DatadogEventSearchParams {
	query: string;
	from: string;
	to: string;
	limit: number;
}

export interface DatadogEventSearchPort {
	searchEvents(
		params: DatadogEventSearchParams,
	): Promise<DatadogEventSearchResult[]>;
}
