import { RequestError } from "@octokit/request-error";
import { GitHubClientError } from "../../github/github-client-error";

export interface GitHubToolSoftError {
	ok: false;
	kind: "client_error";
	message: string;
}

export function toGitHubToolSoftError(
	error: unknown,
): GitHubToolSoftError | undefined {
	const normalizedError =
		error === null || error === undefined ? {} : Object(error);

	if (normalizedError instanceof GitHubClientError) {
		return {
			ok: false,
			kind: "client_error",
			message: normalizedError.message,
		};
	}

	if (
		normalizedError instanceof RequestError &&
		isClientStatusCode(normalizedError.status)
	) {
		return {
			ok: false,
			kind: "client_error",
			message: normalizedError.message,
		};
	}

	return undefined;
}

function isClientStatusCode(statusCode: number): boolean {
	return Number.isInteger(statusCode) && statusCode >= 400 && statusCode <= 499;
}
