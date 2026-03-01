export const MAX_DATADOG_TOOL_RESULT_JSON_LENGTH = 20_000;
export const DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE =
	"Result is too large. Please narrow the time range and try again.";

export interface DatadogToolResultTooLarge {
	ok: false;
	kind: "payload_too_large";
	message: string;
}

interface SerializeDatadogToolResultWithSizeGuardInput<TResult> {
	result: TResult;
	maxJsonLength?: number;
	tooLargeMessage?: string;
}

export function serializeDatadogToolResultWithSizeGuard<TResult>(
	input: SerializeDatadogToolResultWithSizeGuardInput<TResult>,
): string {
	const serializedResult = JSON.stringify(input.result);
	const maxJsonLength =
		input.maxJsonLength ?? MAX_DATADOG_TOOL_RESULT_JSON_LENGTH;
	if (serializedResult.length <= maxJsonLength) {
		return serializedResult;
	}

	const tooLargeResult: DatadogToolResultTooLarge = {
		ok: false,
		kind: "payload_too_large",
		message: input.tooLargeMessage ?? DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE,
	};
	return JSON.stringify(tooLargeResult);
}
