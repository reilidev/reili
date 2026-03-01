import { v2 } from "@datadog/datadog-api-client";
import type {
	DatadogMetricQueryParams,
	DatadogMetricQueryPort,
	DatadogMetricQueryResult,
} from "../../../ports/outbound/datadog-metric-query";
import type { DatadogClientConfig } from "./create-datadog-client-config";

export class DatadogMetricQueryAdapter implements DatadogMetricQueryPort {
	private readonly metricsApi: v2.MetricsApi;

	constructor(datadogClientConfig: DatadogClientConfig) {
		this.metricsApi = new v2.MetricsApi(datadogClientConfig);
	}

	async queryMetrics({
		query,
		from,
		to,
	}: DatadogMetricQueryParams): Promise<DatadogMetricQueryResult[]> {
		const fromMs = toEpochMs(from);
		const toMs = toEpochMs(to);
		validateTimeRange({ fromMs, toMs });
		const response = await this.metricsApi.queryTimeseriesData({
			body: {
				data: {
					type: "timeseries_request",
					attributes: {
						from: fromMs,
						to: toMs,
						queries: [
							{
								dataSource: "metrics",
								query,
							},
						],
					},
				},
			},
		});
		const responseAttributes = response.data?.attributes;
		const times = responseAttributes?.times ?? [];
		const values = responseAttributes?.values ?? [];

		return (responseAttributes?.series ?? []).map((series, seriesIndex) =>
			mapSeriesToMetricQueryResult({
				series,
				seriesIndex,
				times,
				values,
			}),
		);
	}
}

type MetricPoint = [number, number];
type MetricValues = Array<number | null>;

interface MapSeriesToMetricQueryResultInput {
	series: v2.TimeseriesResponseSeries;
	seriesIndex: number;
	times: Array<number>;
	values: Array<MetricValues>;
}

export function mapSeriesToMetricQueryResult(
	input: MapSeriesToMetricQueryResultInput,
): DatadogMetricQueryResult {
	const seriesValues = getSeriesValues({
		values: input.values,
		seriesIndex: input.seriesIndex,
		queryIndex: input.series.queryIndex,
	});
	const unit = toUnitLabel(input.series.unit);
	const points = getMetricPoints(input.times, seriesValues);
	const groupTags = input.series.groupTags?.length
		? input.series.groupTags
		: undefined;

	const metric: DatadogMetricQueryResult = {
		points: points.map(([timestampMs, value]) => ({
			time: toIsoString(timestampMs),
			v: value,
		})),
	};

	if (unit) {
		metric.unit = unit;
	}

	if (groupTags) {
		metric.groupTags = groupTags;
	}

	return metric;
}

function getMetricPoints(times: unknown, values: unknown): MetricPoint[] {
	if (!Array.isArray(times) || !Array.isArray(values)) {
		return [];
	}

	const points: MetricPoint[] = [];
	const maxIndex = Math.min(times.length, values.length) - 1;
	for (let index = maxIndex; index >= 0; index -= 1) {
		const timestamp = times[index];
		const value = values[index];
		if (!Number.isFinite(timestamp) || !Number.isFinite(value)) {
			continue;
		}

		points.push([timestamp, value]);
	}

	return points;
}

interface GetSeriesValuesInput {
	values: Array<MetricValues>;
	seriesIndex: number;
	queryIndex: number | undefined;
}

function getSeriesValues(
	input: GetSeriesValuesInput,
): MetricValues | undefined {
	const preferred = input.values[input.seriesIndex];
	if (preferred) {
		return preferred;
	}

	const queryIndex = input.queryIndex;
	if (queryIndex !== undefined && Number.isInteger(queryIndex)) {
		return input.values[queryIndex] ?? input.values[0];
	}

	return input.values[0];
}

function toIsoString(timestampMs: number): string {
	const date = new Date(timestampMs);
	if (Number.isNaN(date.getTime())) {
		return "unknown";
	}

	return date.toISOString();
}

function toUnitLabel(
	unit?: v2.TimeseriesResponseSeries["unit"],
): string | undefined {
	if (!unit) {
		return undefined;
	}

	const primary = toSingleUnitLabel(unit[0]);
	const per = toSingleUnitLabel(unit[1]);

	if (!primary) {
		return undefined;
	}

	if (!per) {
		return primary;
	}

	return `${primary}/${per}`;
}

function toSingleUnitLabel(value?: v2.Unit | null): string | undefined {
	if (!value) {
		return undefined;
	}

	const shortName = value.shortName;
	if (shortName && shortName.length > 0) {
		return shortName;
	}

	const name = value.name;
	if (name && name.length > 0) {
		return name;
	}

	return undefined;
}

interface ValidateTimeRangeInput {
	fromMs: number;
	toMs: number;
}

function validateTimeRange(input: ValidateTimeRangeInput): void {
	if (input.fromMs > input.toMs) {
		throw new Error('"from" must be earlier than or equal to "to"');
	}
}

function toEpochMs(value: string): number {
	const parsed = Date.parse(value);
	if (!Number.isFinite(parsed)) {
		throw new Error(
			`Invalid time value: "${value}". Use an ISO 8601 string like "2020-10-07T00:00:00+00:00".`,
		);
	}

	return parsed;
}
