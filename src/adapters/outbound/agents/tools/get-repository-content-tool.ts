import { tool } from "@openai/agents";
import { z } from "zod";
import type { InvestigationContext } from "../investigation-agents";
import { assertGithubOwnerInScope } from "./assert-github-owner-in-scope";
import { toGitHubToolSoftError } from "./github-tool-soft-error";
import { requireInvestigationContext } from "./require-investigation-context";

const getRepositoryContentParams = z.object({
	owner: z
		.string()
		.describe("Repository owner (must match configured organization)"),
	repo: z.string().describe("Repository name"),
	path: z.string().describe("File path within the repository"),
	ref: z
		.string()
		.nullable()
		.default(null)
		.describe(
			"Git ref (branch, tag, or commit SHA). Defaults to default branch.",
		),
});

export const getRepositoryContentTool = tool<
	typeof getRepositoryContentParams,
	InvestigationContext
>({
	name: "get_repository_content",
	description: `Retrieve repository content in configured organization scope. Returns kind=file|directory and truncates oversized file content.`,
	parameters: getRepositoryContentParams,
	execute: async (input, context) => {
		try {
			const investigationContext = requireInvestigationContext(context);
			assertGithubOwnerInScope({
				owner: input.owner,
				scopeOrg: investigationContext.resources.githubScopeOrg,
			});
			const port = investigationContext.resources.githubSearchPort;
			const result = await port.getRepositoryContent({
				owner: input.owner,
				repo: input.repo,
				path: input.path,
				ref: input.ref ?? undefined,
			});
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
