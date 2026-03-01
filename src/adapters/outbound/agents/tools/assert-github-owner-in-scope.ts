import { createGitHubClientError } from "../../github/github-client-error";

interface AssertGithubOwnerInScopeInput {
	owner: string;
	scopeOrg: string;
}

export function assertGithubOwnerInScope(
	input: AssertGithubOwnerInScopeInput,
): void {
	if (input.owner.toLowerCase() !== input.scopeOrg.toLowerCase()) {
		throw createGitHubClientError({
			message: `owner is out of scope. allowed owner: ${input.scopeOrg}`,
		});
	}
}
