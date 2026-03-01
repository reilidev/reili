import { RequestError } from "@octokit/request-error";
import { describe, expect, it } from "vitest";
import { createGitHubClientError } from "../../github/github-client-error";
import { toGitHubToolSoftError } from "./github-tool-soft-error";

interface CreateRequestErrorInput {
	message: string;
	statusCode: number;
	responseDataMessage: string;
}

function createRequestError(input: CreateRequestErrorInput): RequestError {
	const requestUrl = "https://api.github.com/search/code";
	const options = {
		request: {
			method: "GET" as const,
			url: requestUrl,
			headers: {},
		},
		response: {
			status: input.statusCode,
			url: requestUrl,
			headers: {},
			data: {
				message: input.responseDataMessage,
			},
			retryCount: 0,
		},
	} as ConstructorParameters<typeof RequestError>[2];

	return new RequestError(input.message, input.statusCode, options);
}

describe("toGitHubToolSoftError", () => {
	it("returns soft error for GitHub API client errors", () => {
		const error = createRequestError({
			message: "Validation Failed",
			statusCode: 422,
			responseDataMessage: "Validation Failed",
		});

		const actual = toGitHubToolSoftError(error);

		expect(actual).toEqual({
			ok: false,
			kind: "client_error",
			message: "Validation Failed",
		});
	});

	it("returns soft error without statusCode for local input validation errors", () => {
		const error = createGitHubClientError({
			message: "org qualifier is required. include org:example",
		});

		const actual = toGitHubToolSoftError(error);

		expect(actual).toEqual({
			ok: false,
			kind: "client_error",
			message: "org qualifier is required. include org:example",
		});
	});

	it("returns undefined for non-client errors", () => {
		const error = createRequestError({
			message: "Internal Server Error",
			statusCode: 500,
			responseDataMessage: "Internal Server Error",
		});

		const actual = toGitHubToolSoftError(error);

		expect(actual).toBeUndefined();
	});
});
