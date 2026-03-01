import type {
	AppendSlackProgressStreamInput,
	SlackAnyChunk,
	SlackProgressStreamPort,
	SlackStreamBlock,
	StartSlackProgressStreamInput,
	StopSlackProgressStreamInput,
} from "../../../ports/outbound/slack-progress-stream";

interface SlackStartStreamResponse {
	ts?: string;
}

type SlackChatApi = {
	startStream(args: {
		channel: string;
		thread_ts: string;
		recipient_user_id: string;
		recipient_team_id?: string;
		markdown_text?: string;
		chunks?: SlackAnyChunk[];
		task_display_mode?: "plan" | "timeline";
	}): Promise<SlackStartStreamResponse>;
	appendStream(args: {
		channel: string;
		ts: string;
		markdown_text?: string;
		chunks?: SlackAnyChunk[];
	}): Promise<object>;
	stopStream(args: {
		channel: string;
		ts: string;
		markdown_text?: string;
		chunks?: SlackAnyChunk[];
		blocks?: SlackStreamBlock[];
	}): Promise<object>;
};

type SlackWebClient = {
	chat: SlackChatApi;
};

export class BoltSlackProgressStreamAdapter implements SlackProgressStreamPort {
	constructor(private readonly client: SlackWebClient) {}

	async start(
		input: StartSlackProgressStreamInput,
	): Promise<{ streamTs: string }> {
		if (!input.markdownText && !input.chunks) {
			throw new Error("Slack stream start requires markdownText or chunks");
		}

		const response = await this.client.chat.startStream({
			channel: input.channel,
			thread_ts: input.threadTs,
			recipient_user_id: input.recipientUserId,
			recipient_team_id: input.recipientTeamId,
			markdown_text: input.markdownText,
			chunks: input.chunks,
			task_display_mode: "plan",
		});
		const streamTs = response.ts;
		if (!streamTs) {
			throw new Error("Slack stream start response did not contain ts");
		}

		return { streamTs };
	}

	async append(input: AppendSlackProgressStreamInput): Promise<void> {
		await this.client.chat.appendStream({
			channel: input.channel,
			ts: input.streamTs,
			markdown_text: input.markdownText,
			chunks: input.chunks,
		});
	}

	async stop(input: StopSlackProgressStreamInput): Promise<void> {
		await this.client.chat.stopStream({
			channel: input.channel,
			ts: input.streamTs,
			markdown_text: input.markdownText,
			chunks: input.chunks,
			blocks: input.blocks,
		});
	}
}
