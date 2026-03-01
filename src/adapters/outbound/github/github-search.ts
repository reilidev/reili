import { createAppAuth } from "@octokit/auth-app";
import { retry } from "@octokit/plugin-retry";
import { throttling } from "@octokit/plugin-throttling";
import { Octokit } from "@octokit/rest";
import Bottleneck from "bottleneck";
import type {
	GithubCodeSearchResultItem,
	GithubIssueSearchResultItem,
	GithubPullRequestDiff,
	GithubPullRequestParams,
	GithubPullRequestSummary,
	GithubRepoSearchResultItem,
	GithubRepositoryContent,
	GithubRepositoryContentParams,
	GithubSearchParams,
	GithubSearchPort,
} from "../../../ports/outbound/github-search";
import { createGitHubClientError } from "./github-client-error";

export interface GitHubSearchAdapterConfig {
	appId: string;
	privateKey: string;
	installationId: number;
	scopeOrg: string;
}

const MAX_RETRY_COUNT = 3;
const MAX_CONTENT_BYTES = 200_000;
const MAX_CONTENT_CHARS = 50_000;
const MAX_DIFF_CHARS = 120_000;

const ThrottledOctokit = Octokit.plugin(throttling, retry);

export class GitHubSearchAdapter implements GithubSearchPort {
	private readonly octokit: InstanceType<typeof ThrottledOctokit>;
	private readonly config: GitHubSearchAdapterConfig;

	constructor(config: GitHubSearchAdapterConfig) {
		this.config = config;
		this.octokit = new ThrottledOctokit({
			authStrategy: createAppAuth,
			auth: {
				appId: config.appId,
				privateKey: config.privateKey,
				installationId: config.installationId,
			},
			throttle: {
				search: new Bottleneck.Group({
					id: `octokit-search-${config.installationId}`,
					maxConcurrent: 3,
					minTime: 2000,
				}),
				onRateLimit: (_retryAfter, _options, _octokit, retryCount) => {
					return retryCount < MAX_RETRY_COUNT;
				},
				onSecondaryRateLimit: (_retryAfter, _options, _octokit, retryCount) => {
					return retryCount < MAX_RETRY_COUNT;
				},
			},
			retry: {
				doNotRetry: [429],
			},
		});
	}

	async searchCode(
		params: GithubSearchParams,
	): Promise<GithubCodeSearchResultItem[]> {
		const scopedQuery = this.requireOrgScopedQuery(params.query);
		const response = await this.octokit.rest.search.code({
			q: scopedQuery,
			per_page: Math.min(params.limit, 30),
		});

		return response.data.items.map((item) => ({
			name: item.name,
			path: item.path,
			repositoryFullName: item.repository.full_name,
			htmlUrl: item.html_url,
		}));
	}

	async searchRepos(
		params: GithubSearchParams,
	): Promise<GithubRepoSearchResultItem[]> {
		const scopedQuery = this.requireOrgScopedQuery(params.query);
		const response = await this.octokit.rest.search.repos({
			q: scopedQuery,
			per_page: Math.min(params.limit, 30),
		});

		return response.data.items.map((item) => ({
			fullName: item.full_name,
			description: item.description ?? null,
			htmlUrl: item.html_url,
			defaultBranch: item.default_branch,
			language: item.language ?? null,
			updatedAt: item.updated_at,
		}));
	}

	async searchIssuesAndPullRequests(
		params: GithubSearchParams,
	): Promise<GithubIssueSearchResultItem[]> {
		const scopedQuery = this.requireOrgScopedQuery(params.query);
		const response = await this.octokit.rest.search.issuesAndPullRequests({
			q: scopedQuery,
			per_page: Math.min(params.limit, 30),
		});

		return response.data.items.map((item) => ({
			number: item.number,
			title: item.title,
			state: item.state,
			htmlUrl: item.html_url,
			repositoryUrl: item.repository_url,
			userLogin: item.user?.login ?? null,
			createdAt: item.created_at,
			updatedAt: item.updated_at,
			pullRequest: item.pull_request !== undefined,
		}));
	}

	async getRepositoryContent(
		params: GithubRepositoryContentParams,
	): Promise<GithubRepositoryContent> {
		const response = await this.octokit.rest.repos.getContent({
			owner: params.owner,
			repo: params.repo,
			path: params.path,
			ref: params.ref,
		});

		const data = response.data;
		if (Array.isArray(data)) {
			return {
				kind: "directory",
				htmlUrl: "",
				entries: data.map((entry) => ({
					name: entry.name,
					path: entry.path,
					type: entry.type,
				})),
			};
		}

		if (data.type !== "file") {
			throw createGitHubClientError({
				message: `Repository content was retrieved, but content type is not supported: ${data.type} at ${params.path}`,
			});
		}

		const decoded = Buffer.from(data.content ?? "", "base64").toString("utf-8");
		const originalBytes = Buffer.byteLength(decoded, "utf-8");
		const byBytes = Buffer.from(decoded, "utf-8")
			.subarray(0, MAX_CONTENT_BYTES)
			.toString("utf-8");
		const truncatedContent =
			byBytes.length > MAX_CONTENT_CHARS
				? byBytes.slice(0, MAX_CONTENT_CHARS)
				: byBytes;

		return {
			kind: "file",
			content: truncatedContent,
			encoding: "utf-8",
			htmlUrl: data.html_url ?? "",
			originalBytes,
			returnedChars: truncatedContent.length,
			truncated:
				originalBytes > MAX_CONTENT_BYTES || byBytes.length > MAX_CONTENT_CHARS,
		};
	}

	async getPullRequest(
		params: GithubPullRequestParams,
	): Promise<GithubPullRequestSummary> {
		const response = await this.octokit.rest.pulls.get({
			owner: params.owner,
			repo: params.repo,
			pull_number: params.pullNumber,
		});

		const pr = response.data;
		return {
			number: pr.number,
			state: pr.state,
			title: pr.title,
			body: pr.body ?? null,
			userLogin: pr.user?.login,
			createdAt: pr.created_at,
			updatedAt: pr.updated_at,
			mergedAt: pr.merged_at ?? undefined,
			additions: pr.additions,
			deletions: pr.deletions,
			changedFiles: pr.changed_files,
			commits: pr.commits,
			htmlUrl: pr.html_url,
			baseRef: pr.base.ref,
			headRef: pr.head.ref,
		};
	}

	async getPullRequestDiff(
		params: GithubPullRequestParams,
	): Promise<GithubPullRequestDiff> {
		const response = await this.octokit.rest.pulls.get({
			owner: params.owner,
			repo: params.repo,
			pull_number: params.pullNumber,
			mediaType: { format: "diff" },
		});
		const rawDiff = String(response.data);
		const diff =
			rawDiff.length > MAX_DIFF_CHARS
				? `${rawDiff.slice(0, MAX_DIFF_CHARS)}\n\n... [truncated]`
				: rawDiff;

		return {
			diff,
			htmlUrl: `https://github.com/${params.owner}/${params.repo}/pull/${params.pullNumber}`,
			originalChars: rawDiff.length,
			returnedChars: diff.length,
			truncated: rawDiff.length > MAX_DIFF_CHARS,
		};
	}

	private requireOrgScopedQuery(query: string): string {
		const orgQualifierPattern = /(^|\s)org:([^\s]+)/gi;
		const qualifiers = Array.from(query.matchAll(orgQualifierPattern)).map(
			(match) => match[2].toLowerCase(),
		);
		const targetOrg = this.config.scopeOrg.toLowerCase();

		if (qualifiers.length === 0) {
			throw createGitHubClientError({
				message: `org qualifier is required. include org:${this.config.scopeOrg}`,
			});
		}
		if (qualifiers.some((org) => org !== targetOrg)) {
			throw createGitHubClientError({
				message: `org qualifier is out of scope. allowed org: ${this.config.scopeOrg}`,
			});
		}
		return query;
	}
}
