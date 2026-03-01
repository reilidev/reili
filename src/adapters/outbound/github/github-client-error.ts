export class GitHubClientError extends Error {
	constructor(message: string) {
		super(message);
		this.name = "GitHubClientError";
	}
}

export function createGitHubClientError(input: {
	message: string;
}): GitHubClientError {
	return new GitHubClientError(input.message);
}
