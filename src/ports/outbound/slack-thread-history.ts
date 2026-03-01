import type { SlackThreadMessage } from "../../shared/types/slack-thread-message";

export interface FetchSlackThreadHistoryInput {
	channel: string;
	threadTs: string;
}

export interface SlackThreadHistoryPort {
	fetchThreadHistory(
		input: FetchSlackThreadHistoryInput,
	): Promise<SlackThreadMessage[]>;
}
