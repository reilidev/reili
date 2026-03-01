import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { toGitHubToolSoftError } from "./github-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export function createSearchGithubReposTool() {
	const searchGithubReposParams = z.object({
		query: z
			.string()
			.describe(
				"GitHub repository search query (e.g. 'service-a org:<githubScopeOrg>', 'language:go topic:microservice org:<githubScopeOrg>'). Always include org:<githubScopeOrg from runtime context>.",
			),
		limit: z
			.number()
			.int()
			.min(1)
			.max(30)
			.default(10)
			.describe("Maximum number of results"),
	});

	return tool<typeof searchGithubReposParams, InvestigationContext>({
		name: "search_github_repos",
		description:
			"Search GitHub repositories inside the configured organization scope. Every query must explicitly include org:<githubScopeOrg from runtime context>. Returns repository metadata (name, description, language, default branch, etc.).",
		parameters: searchGithubReposParams,
		execute: async (toolInput, context) => {
			const port =
				requireInvestigationContext(context).resources.githubSearchPort;
			try {
				const results = await port.searchRepos({
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
