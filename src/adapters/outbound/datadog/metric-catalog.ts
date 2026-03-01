import { v1 } from "@datadog/datadog-api-client";
import type {
	DatadogMetricCatalogParams,
	DatadogMetricCatalogPort,
} from "../../../ports/outbound/datadog-metric-catalog";
import type { DatadogClientConfig } from "./create-datadog-client-config";

export class DatadogMetricCatalogAdapter implements DatadogMetricCatalogPort {
	private readonly metricsV1Api: v1.MetricsApi;

	constructor(datadogClientConfig: DatadogClientConfig) {
		this.metricsV1Api = new v1.MetricsApi(datadogClientConfig);
	}

	async listMetrics(params: DatadogMetricCatalogParams): Promise<string[]> {
		const limit = normalizeLimit(params.limit);
		if (limit === 0) {
			return [];
		}

		const fromEpochSec = normalizeFromEpochSec(params.fromEpochSec);
		const tagFilter = normalizeOptionalText(params.tagFilter);

		const activeMetrics = await this.metricsV1Api.listActiveMetrics({
			from: fromEpochSec,
			tagFilter,
		});

		return (activeMetrics.metrics ?? []).slice(0, limit);
	}
}

function normalizeOptionalText(value: string | undefined): string | undefined {
	const normalized = value?.trim();
	if (!normalized) {
		return undefined;
	}

	return normalized;
}

function normalizeLimit(limit: number): number {
	if (!Number.isFinite(limit)) {
		return 0;
	}

	const normalized = Math.floor(limit);
	if (normalized <= 0) {
		return 0;
	}

	return normalized;
}

function normalizeFromEpochSec(fromEpochSec: number): number {
	if (!Number.isFinite(fromEpochSec)) {
		return 0;
	}

	const normalized = Math.floor(fromEpochSec);
	if (normalized <= 0) {
		return 0;
	}

	return normalized;
}
