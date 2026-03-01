import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { toGitHubToolSoftError } from "./github-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

export function createSearchGithubCodeTool() {
	const searchGithubCodeParams = z.object({
		query: z.string().describe(
			`GitHub Code Search query: bare terms match file content or path;
separate tokens with spaces (implicit AND), use AND/OR/NOT with parentheses;
quote exact strings, use /regex/, and qualifiers like repo:, org:, user:, language:, path:, symbol:, content:, is:, don't use filename:.
Always include org:<githubScopeOrg from runtime context> in this query.`,
		),
		limit: z
			.number()
			.int()
			.min(1)
			.max(30)
			.default(10)
			.describe("Maximum number of results"),
	});

	return tool<typeof searchGithubCodeParams, InvestigationContext>({
		name: "search_github_code",
		description:
			"Search GitHub code inside the configured organization scope. Every query must explicitly include org:<githubScopeOrg from runtime context>.",
		parameters: searchGithubCodeParams,
		execute: async (toolInput, context) => {
			const port =
				requireInvestigationContext(context).resources.githubSearchPort;
			try {
				const results = await port.searchCode({
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
