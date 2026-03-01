import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { toDatadogToolSoftError } from "./datadog-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export const listMetricsCatalogParams = z.object({
	lookbackHours: z
		.number()
		.int()
		.min(1)
		.max(720)
		.default(24)
		.describe("How many hours back to scan for active metrics"),
	tagFilter: z
		.string()
		.default("")
		.describe(
			"Optional Datadog tag filter expression (e.g. env:prod, service:system-*). Supports Datadog boolean and wildcard expressions",
		),
	limit: z
		.number()
		.int()
		.min(1)
		.max(500)
		.default(100)
		.describe("Maximum number of unique metric names to return"),
});

interface ListMetricsCatalogToolResult {
	total: number;
	metrics: string[];
}

export const listMetricsCatalogTool = tool<
	typeof listMetricsCatalogParams,
	InvestigationContext
>({
	name: "list_datadog_metrics_catalog",
	description:
		"List available Datadog metrics for a recent time window. Returns metric names, prefix counts, and representative examples for environment discovery.",
	parameters: listMetricsCatalogParams,
	execute: async (input, context) => {
		const metricCatalogPort =
			requireInvestigationContext(context).resources.metricCatalogPort;
		const nowEpochSec = Math.floor(Date.now() / 1000);
		const fromEpochSec = nowEpochSec - input.lookbackHours * 60 * 60;
		try {
			const metrics = await metricCatalogPort.listMetrics({
				fromEpochSec,
				tagFilter: input.tagFilter ?? undefined,
				limit: input.limit,
			});

			return JSON.stringify({
				total: metrics.length,
				metrics,
			} as ListMetricsCatalogToolResult);
		} catch (error) {
			const softError = toDatadogToolSoftError(error);
			if (softError) {
				return JSON.stringify(softError);
			}

			throw error;
		}
	},
});
