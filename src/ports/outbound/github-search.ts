export interface GithubSearchParams {
	query: string;
	limit: number;
}

export interface GithubRepositoryContentParams {
	owner: string;
	repo: string;
	path: string;
	ref?: string;
}

export interface GithubPullRequestParams {
	owner: string;
	repo: string;
	pullNumber: number;
}

export interface GithubCodeSearchResultItem {
	name: string;
	path: string;
	repositoryFullName: string;
	htmlUrl: string;
}

export interface GithubRepoSearchResultItem {
	fullName: string;
	description: string | null;
	htmlUrl: string;
	defaultBranch: string;
	language: string | null;
	updatedAt: string;
}

export interface GithubIssueSearchResultItem {
	number: number;
	title: string;
	state: string;
	htmlUrl: string;
	repositoryUrl: string;
	userLogin: string | null;
	createdAt: string;
	updatedAt: string;
	pullRequest: boolean;
}

export interface GithubRepositoryFileContent {
	kind: "file";
	content: string;
	encoding: "utf-8";
	htmlUrl: string;
	originalBytes: number;
	returnedChars: number;
	truncated: boolean;
}

export interface GithubRepositoryDirectoryContent {
	kind: "directory";
	htmlUrl: string;
	entries: Array<{
		name: string;
		path: string;
		type: string;
	}>;
}

export type GithubRepositoryContent =
	| GithubRepositoryFileContent
	| GithubRepositoryDirectoryContent;

export interface GithubPullRequestDiff {
	diff: string;
	htmlUrl: string;
	originalChars: number;
	returnedChars: number;
	truncated: boolean;
}

export interface GithubPullRequestSummary {
	number: number;
	state: string;
	title: string;
	body: string | null;
	userLogin?: string;
	createdAt?: string;
	updatedAt?: string;
	mergedAt?: string;
	additions?: number;
	deletions?: number;
	changedFiles?: number;
	commits?: number;
	htmlUrl: string;
	baseRef?: string;
	headRef?: string;
}

export interface GithubSearchPort {
	searchCode(params: GithubSearchParams): Promise<GithubCodeSearchResultItem[]>;

	searchRepos(
		params: GithubSearchParams,
	): Promise<GithubRepoSearchResultItem[]>;

	searchIssuesAndPullRequests(
		params: GithubSearchParams,
	): Promise<GithubIssueSearchResultItem[]>;

	getRepositoryContent(
		params: GithubRepositoryContentParams,
	): Promise<GithubRepositoryContent>;

	getPullRequest(
		params: GithubPullRequestParams,
	): Promise<GithubPullRequestSummary>;

	getPullRequestDiff(
		params: GithubPullRequestParams,
	): Promise<GithubPullRequestDiff>;
}
