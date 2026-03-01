import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { toGitHubToolSoftError } from "./github-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export function createSearchGithubIssuesAndPullRequestsTool() {
	const searchGithubIssuesAndPullRequestsParams = z.object({
		query: z
			.string()
			.describe(
				"GitHub issues/PR search query (e.g. 'is:pr is:merged label:bug org:<githubScopeOrg>', 'is:issue is:open repo:<githubScopeOrg>/repo-name org:<githubScopeOrg>'). Always include org:<githubScopeOrg from runtime context>.",
			),
		limit: z
			.number()
			.int()
			.min(1)
			.max(30)
			.default(10)
			.describe("Maximum number of results"),
	});

	return tool<
		typeof searchGithubIssuesAndPullRequestsParams,
		InvestigationContext
	>({
		name: "search_github_issues_and_pull_requests",
		description:
			"Search GitHub issues and pull requests inside the configured organization scope. Every query must explicitly include org:<githubScopeOrg from runtime context>. Use qualifiers: is:pr, is:issue, is:open, is:merged, label:, repo:, author:, etc.",
		parameters: searchGithubIssuesAndPullRequestsParams,
		execute: async (toolInput, context) => {
			const port =
				requireInvestigationContext(context).resources.githubSearchPort;
			try {
				const results = await port.searchIssuesAndPullRequests({
					query: toolInput.query,
					limit: toolInput.limit,
				});
				return JSON.stringify(results);
			} catch (error) {
				const softError = toGitHubToolSoftError(
					error as null | object | undefined,
				);
				if (softError) {
					return JSON.stringify(softError);
				}

				throw error;
			}
		},
	});
}
