export interface DatadogLogAggregateBucket {
	key: string;
	count: number;
}

export interface DatadogLogAggregateParams {
	query: string;
	from: string;
	to: string;
	facet: string;
	limit: number;
}

export interface DatadogLogAggregatePort {
	aggregateByFacet(
		params: DatadogLogAggregateParams,
	): Promise<DatadogLogAggregateBucket[]>;
}
