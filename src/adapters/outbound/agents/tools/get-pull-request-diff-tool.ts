import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { assertGithubOwnerInScope } from "./assert-github-owner-in-scope";
import { toGitHubToolSoftError } from "./github-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

const getPullRequestDiffParams = z.object({
	owner: z.string().describe("Repository owner"),
	repo: z.string().describe("Repository name"),
	pullNumber: z.number().int().min(1).describe("Pull request number"),
});

export const getPullRequestDiffTool = tool<
	typeof getPullRequestDiffParams,
	InvestigationContext
>({
	name: "get_pull_request_diff",
	description: `Get the code diff of a GitHub pull request in configured organization scope. Diff is truncated when too large; use sparingly after narrowing scope with get_pull_request and search_github_code.`,
	parameters: getPullRequestDiffParams,
	execute: async (input, context) => {
		try {
			const investigationContext = requireInvestigationContext(context);
			assertGithubOwnerInScope({
				owner: input.owner,
				scopeOrg: investigationContext.resources.githubScopeOrg,
			});
			const port = investigationContext.resources.githubSearchPort;
			const result = await port.getPullRequestDiff(input);
			return JSON.stringify(result);
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
