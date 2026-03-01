export type SlackTriggerType = "message" | "app_mention";

export interface SlackMessage {
	slackEventId: string;
	teamId?: string;
	trigger: SlackTriggerType;
	channel: string;
	user: string;
	text: string;
	ts: string;
	threadTs?: string;
}
