export interface DatadogMetricQueryPoint {
	// timestamp
	time: string;

	// value
	v: number;
}

export interface DatadogMetricQueryResult {
	metric?: string;
	unit?: string;
	groupTags?: string[];
	points: DatadogMetricQueryPoint[];
}

export interface DatadogMetricQueryParams {
	query: string;
	from: string;
	to: string;
}

export interface DatadogMetricQueryPort {
	queryMetrics(
		params: DatadogMetricQueryParams,
	): Promise<DatadogMetricQueryResult[]>;
}
