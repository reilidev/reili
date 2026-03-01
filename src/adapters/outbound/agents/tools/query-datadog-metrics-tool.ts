import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { serializeDatadogToolResultWithSizeGuard } from "./datadog-tool-result-size-guard";
import { toDatadogToolSoftError } from "./datadog-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export const queryMetricsParams = z.object({
	query: z.string().describe("Datadog metric query (e.g. avg:system.cpu{*})"),
	from: z.iso
		.datetime({ offset: true })
		.describe(
			'Start time in ISO 8601 format (e.g. "2020-10-07T00:00:00+00:00")',
		),
	to: z.iso
		.datetime({ offset: true })
		.describe('End time in ISO 8601 format (e.g. "2020-10-07T00:15:00+00:00")'),
});

export const queryMetricsTool = tool<
	typeof queryMetricsParams,
	InvestigationContext
>({
	name: "query_datadog_metrics",
	description:
		"Query Datadog timeseries metrics with a query and time range. Returns series with mapped time/value points.",
	parameters: queryMetricsParams,
	execute: async (input, context) => {
		const port = requireInvestigationContext(context).resources.metricQueryPort;
		try {
			const results = await port.queryMetrics({
				query: input.query,
				from: input.from,
				to: input.to,
			});
			return serializeDatadogToolResultWithSizeGuard({
				result: results,
			});
		} catch (error) {
			const softError = toDatadogToolSoftError(error);
			if (softError) {
				return JSON.stringify(softError);
			}

			throw error;
		}
	},
});
