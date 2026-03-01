import { describe, expect, it } from "vitest";
import {
	DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE,
	MAX_DATADOG_TOOL_RESULT_JSON_LENGTH,
	serializeDatadogToolResultWithSizeGuard,
} from "./datadog-tool-result-size-guard";

interface ToolResultItem {
	id: string;
	payload: string;
}

function createToolResult(itemCount: number): ToolResultItem[] {
	return Array.from({ length: itemCount }, (_, index) => ({
		id: `item-${index}`,
		payload: "x".repeat(40),
	}));
}

describe("serializeDatadogToolResultWithSizeGuard", () => {
	it("returns serialized payload when payload size is within limit", () => {
		const result = createToolResult(5);
		const expected = JSON.stringify(result);

		const actual = serializeDatadogToolResultWithSizeGuard({ result });

		expect(expected.length).toBeLessThanOrEqual(
			MAX_DATADOG_TOOL_RESULT_JSON_LENGTH,
		);
		expect(actual).toBe(expected);
	});

	it("returns payload_too_large response when payload size exceeds limit", () => {
		const result = createToolResult(500);
		const oversizedJson = JSON.stringify(result);

		const actual = serializeDatadogToolResultWithSizeGuard({ result });

		expect(oversizedJson.length).toBeGreaterThan(
			MAX_DATADOG_TOOL_RESULT_JSON_LENGTH,
		);
		expect(actual).toBe(
			JSON.stringify({
				ok: false,
				kind: "payload_too_large",
				message: DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE,
			}),
		);
	});
});
