import type { SlackMessage } from "../../shared/types/slack-message";

export interface SlackMessageHandlerPort {
	handle(message: SlackMessage): Promise<void>;
}
