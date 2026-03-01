export interface DatadogMetricCatalogParams {
	fromEpochSec: number;
	tagFilter?: string;
	limit: number;
}

export interface DatadogMetricCatalogPort {
	listMetrics(params: DatadogMetricCatalogParams): Promise<string[]>;
}
