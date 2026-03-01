import type { App } from "@slack/bolt";
import type {
	FetchSlackThreadHistoryInput,
	SlackThreadHistoryPort,
} from "../../../ports/outbound/slack-thread-history";
import type { SlackThreadMessage } from "../../../shared/types/slack-thread-message";

type SlackWebClient = {
	conversations: Pick<App["client"]["conversations"], "replies">;
};

// https://docs.slack.dev/changelog/2025/05/29/rate-limit-changes-for-non-marketplace-apps/
const THREAD_HISTORY_PAGE_LIMIT = 15;
const THREAD_HISTORY_MAX_MESSAGES = 200;

export class SlackThreadHistoryAdapter implements SlackThreadHistoryPort {
	constructor(private readonly client: SlackWebClient) {}

	async fetchThreadHistory(
		input: FetchSlackThreadHistoryInput,
	): Promise<SlackThreadMessage[]> {
		const messages: SlackThreadMessage[] = [];
		let cursor: string | undefined;

		do {
			const response = await this.client.conversations.replies({
				channel: input.channel,
				ts: input.threadTs,
				cursor,
				limit: THREAD_HISTORY_PAGE_LIMIT,
			});
			const pageMessages = response.messages ?? [];
			for (const pageMessage of pageMessages) {
				if (messages.length >= THREAD_HISTORY_MAX_MESSAGES) {
					break;
				}

				const ts = pageMessage.ts;
				if (!ts) {
					continue;
				}

				messages.push({
					ts,
					user: pageMessage.user,
					text: pageMessage.text ?? "",
				});
			}
			cursor = response.response_metadata?.next_cursor;
			if (messages.length >= THREAD_HISTORY_MAX_MESSAGES) {
				break;
			}
		} while (cursor && cursor.length > 0);

		return messages;
	}
}
