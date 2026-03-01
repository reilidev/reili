import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { serializeDatadogToolResultWithSizeGuard } from "./datadog-tool-result-size-guard";
import { toDatadogToolSoftError } from "./datadog-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export const searchLogsParams = z.object({
	query: z.string().describe("Datadog log search query"),
	from: z
		.string()
		.default("now-15m")
		.describe(
			'Start time (date math or ISO string, e.g. "now-15m" or "2020-10-07T00:00:00+00:00")',
		),
	to: z
		.string()
		.default("now")
		.describe(
			'End time (date math or ISO string, e.g. "now" or "2020-10-07T00:15:00+00:00")',
		),
	limit: z.number().int().min(1).max(100).describe("Maximum number of logs"),
});

export const searchLogsTool = tool<
	typeof searchLogsParams,
	InvestigationContext
>({
	name: "search_datadog_logs",
	description:
		"Search Datadog logs with a query and time range. Returns recent log entries matching the query.",
	parameters: searchLogsParams,
	execute: async (input, context) => {
		const port = requireInvestigationContext(context).resources.logSearchPort;
		try {
			const results = await port.searchLogs({
				query: input.query,
				from: input.from,
				to: input.to,
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
