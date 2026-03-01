import type { client } from "@datadog/datadog-api-client";

interface DatadogApiErrorItem {
	detail?: string;
	title?: string;
}

interface DatadogApiErrorBody {
	errors?: DatadogApiErrorItem[];
	message?: string;
}

type DatadogApiExceptionBody = DatadogApiErrorBody | string;

export interface DatadogToolSoftError {
	ok: false;
	kind: "client_error";
	statusCode: number;
	message: string;
}

export function toDatadogToolSoftError(
	error: unknown,
): DatadogToolSoftError | undefined {
	console.error("toDatadogToolSoftError", error);
	const normalizedError =
		error === null || error === undefined ? {} : Object(error);
	const apiException =
		normalizedError as client.ApiException<DatadogApiExceptionBody>;
	const statusCode = Number(apiException.code);
	if (!Number.isInteger(statusCode) || statusCode < 400 || statusCode > 499) {
		return undefined;
	}
	return {
		ok: false,
		kind: "client_error",
		statusCode,
		message: normalizedError.toString(),
	};
	//
	// const body = apiException.body;
	// let message = "Datadog client error";
	// if (Object.prototype.toString.call(body) === "[object String]") {
	// 	const text = (body as string).trim();
	// 	if (text.length > 0) {
	// 		message = text;
	// 	}
	// } else if (Object.prototype.toString.call(body) === "[object Object]") {
	// 	const typedBody = body as DatadogApiErrorBody;
	// 	const detail = typedBody.errors?.[0]?.detail?.trim();
	// 	if (detail) {
	// 		message = detail;
	// 	} else {
	// 		const title = typedBody.errors?.[0]?.title?.trim();
	// 		if (title) {
	// 			message = title;
	// 		} else {
	// 			const fallback = typedBody.message?.trim();
	// 			if (fallback) {
	// 				message = fallback;
	// 			}
	// 		}
	// 	}
	// }

	// return {
	// 	ok: false,
	// 	kind: "client_error",
	// 	statusCode,
	// 	message,
	// };
}
