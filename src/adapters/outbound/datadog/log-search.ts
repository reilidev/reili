import { v2 } from "@datadog/datadog-api-client";
import type {
	DatadogLogSearchParams,
	DatadogLogSearchPort,
	DatadogLogSearchResult,
} from "../../../ports/outbound/datadog-log-search";
import type { DatadogClientConfig } from "./create-datadog-client-config";

export class DatadogLogSearchAdapter implements DatadogLogSearchPort {
	private readonly logsApi: v2.LogsApi;

	constructor(datadogClientConfig: DatadogClientConfig) {
		this.logsApi = new v2.LogsApi(datadogClientConfig);
	}

	async searchLogs(
		params: DatadogLogSearchParams,
	): Promise<DatadogLogSearchResult[]> {
		const response = await this.logsApi.listLogs({
			body: {
				filter: {
					query: params.query,
					from: params.from,
					to: params.to,
				},
				sort: "-timestamp",
				page: {
					limit: params.limit,
				},
			},
		});

		return (response.data ?? []).map((log) => ({
			id: log.id ?? "",
			timestamp: toIsoString(log.attributes?.timestamp),
			service: log.attributes?.service,
			status: log.attributes?.status,
			message: log.attributes?.message,
			attributes: toAttributesRecord(log.attributes?.attributes),
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

function toAttributesRecord(
	attributes: unknown,
): Record<string, unknown> | undefined {
	if (!attributes || typeof attributes !== "object") {
		return undefined;
	}

	return attributes as Record<string, unknown>;
}
