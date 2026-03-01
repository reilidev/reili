export interface SlackMarkdownTextChunk {
	type: "markdown_text";
	text: string;
}

export interface SlackPlanUpdateChunk {
	type: "plan_update";
	title: string;
}

export interface SlackTaskUpdateChunk {
	type: "task_update";
	id: string;
	title: string;
	status: "pending" | "in_progress" | "complete" | "error";
	details?: string;
	output?: string;
	sources?: {
		type: "url";
		url: string;
		text: string;
	}[];
}

export type SlackAnyChunk =
	| SlackMarkdownTextChunk
	| SlackPlanUpdateChunk
	| SlackTaskUpdateChunk;

export interface SlackStreamBlock {
	type: string;
	block_id?: string;
}

export interface StartSlackProgressStreamInput {
	channel: string;
	threadTs: string;
	recipientUserId: string;
	recipientTeamId?: string;
	markdownText?: string;
	chunks?: SlackAnyChunk[];
}

export interface AppendSlackProgressStreamInput {
	channel: string;
	streamTs: string;
	markdownText?: string;
	chunks?: SlackAnyChunk[];
}

export interface StopSlackProgressStreamInput {
	channel: string;
	streamTs: string;
	markdownText?: string;
	chunks?: SlackAnyChunk[];
	blocks?: SlackStreamBlock[];
}

export interface SlackProgressStreamPort {
	start(input: StartSlackProgressStreamInput): Promise<{ streamTs: string }>;
	append(input: AppendSlackProgressStreamInput): Promise<void>;
	stop(input: StopSlackProgressStreamInput): Promise<void>;
}
