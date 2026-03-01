import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { serializeDatadogToolResultWithSizeGuard } from "./datadog-tool-result-size-guard";
import { toDatadogToolSoftError } from "./datadog-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export const aggregateLogsByFacetParams = z.object({
	query: z
		.string()
		.default("*")
		.describe("Datadog log search query used before aggregation"),
	from: z
		.string()
		.default("now-30m")
		.describe('Start time (date math or ISO string, e.g. "now-30m")'),
	to: z
		.string()
		.default("now")
		.describe('End time (date math or ISO string, e.g. "now")'),
	facet: z
		.string()
		.default("service")
		.describe("Facet name used for aggregation. Defaults to service."),
	limit: z
		.number()
		.int()
		.min(1)
		.max(50)
		.default(20)
		.describe("Maximum number of buckets to return"),
});

export const aggregateLogsByFacetTool = tool<
	typeof aggregateLogsByFacetParams,
	InvestigationContext
>({
	name: "aggregate_datadog_logs_by_facet",
	description:
		"Aggregate Datadog logs by facet and return top buckets by count. Use this to discover active services early in an investigation.",
	parameters: aggregateLogsByFacetParams,
	execute: async (input, context) => {
		const port =
			requireInvestigationContext(context).resources.logAggregatePort;
		try {
			const results = await port.aggregateByFacet({
				query: input.query,
				from: input.from,
				to: input.to,
				facet: input.facet,
				limit: input.limit,
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
