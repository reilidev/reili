import type {
	SlackThreadReplyInput,
	SlackThreadReplyPort,
} from "../../../ports/outbound/slack-thread-reply";

type SlackWebClient = {
	chat: {
		postMessage(args: {
			channel: string;
			thread_ts: string;
			markdown_text: string;
		}): Promise<unknown>;
	};
};

export class BoltSlackThreadReplyAdapter implements SlackThreadReplyPort {
	constructor(private readonly client: SlackWebClient) {}

	async postThreadReply(input: SlackThreadReplyInput): Promise<void> {
		await this.client.chat.postMessage({
			channel: input.channel,
			thread_ts: input.threadTs,
			markdown_text: input.text,
		});
	}
}
