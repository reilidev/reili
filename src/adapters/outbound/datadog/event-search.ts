import { v2 } from "@datadog/datadog-api-client";
import type {
	DatadogEventSearchParams,
	DatadogEventSearchPort,
	DatadogEventSearchResult,
} from "../../../ports/outbound/datadog-event-search";
import type { DatadogClientConfig } from "./create-datadog-client-config";

export class DatadogEventSearchAdapter implements DatadogEventSearchPort {
	private readonly eventsApi: v2.EventsApi;

	constructor(datadogClientConfig: DatadogClientConfig) {
		this.eventsApi = new v2.EventsApi(datadogClientConfig);
	}

	async searchEvents(
		params: DatadogEventSearchParams,
	): Promise<DatadogEventSearchResult[]> {
		const nowMs = Date.now();
		const fromMs = resolveTimeExpressionToEpochMs(params.from, nowMs);
		const toMs = resolveTimeExpressionToEpochMs(params.to, nowMs);
		validateTimeRange({ fromMs, toMs });
		const response = await this.eventsApi.listEvents({
			filterQuery: params.query,
			filterFrom: new Date(fromMs).toISOString(),
			filterTo: new Date(toMs).toISOString(),
			sort: "-timestamp",
			pageLimit: params.limit,
		});

		return (response.data ?? []).map((event) => ({
			id: event.id ?? "",
			timestamp: toIsoString(event.attributes?.timestamp),
			source: event.attributes?.attributes?.sourceTypeName,
			status: toStringValue(event.attributes?.attributes?.status),
			title: event.attributes?.attributes?.title,
			message: event.attributes?.message,
			tags: event.attributes?.tags,
		}));
	}
}

function toIsoString(timestamp: unknown): string {
	if (timestamp instanceof Date) {
		return timestamp.toISOString();
	}

	if (typeof timestamp === "string") {
		return timestamp;
	}

	return "unknown";
}

function toStringValue(value: unknown): string | undefined {
	if (typeof value === "string") {
		return value;
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

function resolveTimeExpressionToEpochMs(
	value: string,
	referenceNowMs: number,
): number {
	if (value === "now") {
		return referenceNowMs;
	}

	const relativeMatch = value.match(/^now([+-])(\d+)([smhdw])$/);
	if (relativeMatch) {
		const sign = relativeMatch[1];
		const amount = Number(relativeMatch[2]);
		const unit = relativeMatch[3];
		const offsetMs = convertDurationToMs(amount, unit);
		if (sign === "-") {
			return referenceNowMs - offsetMs;
		}

		return referenceNowMs + offsetMs;
	}

	const parsed = Date.parse(value);
	if (!Number.isFinite(parsed)) {
		throw new Error(
			`Invalid time expression: "${value}". Use date math like "now-15m" or an ISO 8601 string like "2020-10-07T00:00:00+00:00".`,
		);
	}

	return parsed;
}

function convertDurationToMs(amount: number, unit: string): number {
	if (unit === "s") {
		return amount * 1000;
	}

	if (unit === "m") {
		return amount * 60 * 1000;
	}

	if (unit === "h") {
		return amount * 60 * 60 * 1000;
	}

	if (unit === "d") {
		return amount * 24 * 60 * 60 * 1000;
	}

	return amount * 7 * 24 * 60 * 60 * 1000;
}
