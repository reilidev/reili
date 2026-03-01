import { v2 } from "@datadog/datadog-api-client";
import type {
	DatadogLogAggregateBucket,
	DatadogLogAggregateParams,
	DatadogLogAggregatePort,
} from "../../../ports/outbound/datadog-log-aggregate";
import type { DatadogClientConfig } from "./create-datadog-client-config";

const DEFAULT_FACET = "service";

export class DatadogLogAggregateAdapter implements DatadogLogAggregatePort {
	private readonly logsApi: v2.LogsApi;

	constructor(datadogClientConfig: DatadogClientConfig) {
		this.logsApi = new v2.LogsApi(datadogClientConfig);
	}

	async aggregateByFacet(
		params: DatadogLogAggregateParams,
	): Promise<DatadogLogAggregateBucket[]> {
		const facet = normalizeFacet(params.facet);
		const response = await this.logsApi.aggregateLogs({
			body: {
				filter: {
					query: params.query,
					from: params.from,
					to: params.to,
				},
				compute: [
					{
						aggregation: "count",
					},
				],
				groupBy: [
					{
						facet,
						limit: params.limit,
						sort: {
							aggregation: "count",
							order: "desc",
							type: "measure",
						},
					},
				],
			},
		});

		return (response.data?.buckets ?? []).flatMap((bucket) => {
			const key = toBucketKey(readBucketFacetValue(bucket.by, facet));
			const count = extractCount(bucket.computes);
			if (!key || count === undefined) {
				return [];
			}

			return [{ key, count }];
		});
	}
}

function normalizeFacet(facet: string): string {
	const normalized = facet.trim();
	if (normalized.length === 0) {
		return DEFAULT_FACET;
	}

	return normalized;
}

function toBucketKey(value: unknown): string | undefined {
	if (value === null || value === undefined) {
		return undefined;
	}

	const key = String(value).trim();
	if (key.length === 0) {
		return undefined;
	}

	return key;
}

function readBucketFacetValue(by: unknown, facet: string): unknown | undefined {
	if (by === null || by === undefined) {
		return undefined;
	}

	const values = by as Record<string, unknown>;
	for (const key of buildFacetKeyCandidates(facet)) {
		const value = values[key];
		if (value !== null && value !== undefined) {
			return value;
		}
	}

	return undefined;
}

function buildFacetKeyCandidates(facet: string): string[] {
	if (facet.startsWith("@")) {
		return [facet, facet.slice(1)];
	}

	return [facet, `@${facet}`];
}

function extractCount(computes: unknown): number | undefined {
	if (computes === null || computes === undefined) {
		return undefined;
	}

	const values = computes as Record<string, unknown>;
	const countByName = toCount(values.count);
	if (countByName !== undefined) {
		return countByName;
	}

	for (const value of Object.values(values)) {
		const count = toCount(value);
		if (count !== undefined) {
			return count;
		}
	}

	return undefined;
}

function toCount(value: unknown): number | undefined {
	if (value === null || value === undefined) {
		return undefined;
	}

	const count = Number(value);
	if (!Number.isFinite(count)) {
		return undefined;
	}

	return count;
}
