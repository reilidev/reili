import type { App, EventFromType } from "@slack/bolt";
import { z } from "zod";
import type { SlackMessageHandlerPort } from "../../../ports/inbound/slack-message-handler";
import type { Logger } from "../../../shared/observability/logger";
import { toErrorMessage } from "../../../shared/utils/to-error-message";

type MessageEvent = EventFromType<"message">;
type AppMentionEvent = EventFromType<"app_mention">;

const slackEventEnvelopeSchema = z.object({
	event_id: z.string().min(1),
	team_id: z.string().min(1).optional(),
});

function isProcessableMessageEvent(
	event: MessageEvent,
): event is MessageEvent & {
	subtype: undefined;
	user: string;
	text: string;
	channel: string;
	ts: string;
	thread_ts?: string;
} {
	return event.type === "message" && event.subtype === undefined;
}

function isProcessableMentionEvent(
	event: AppMentionEvent,
): event is AppMentionEvent & {
	user: string;
	text: string;
	channel: string;
	ts: string;
	thread_ts?: string;
} {
	return typeof event.user === "string";
}

export function registerSlackEventHandlers(
	app: App,
	handler: SlackMessageHandlerPort,
	logger: Logger,
): void {
	app.event("message", async ({ event, context, body }) => {
		if (!isProcessableMessageEvent(event)) {
			logger.warn("Received unexpected message event");
			return;
		}
		if (context.botUserId === event.user) {
			return;
		}
		const parsedEnvelope = slackEventEnvelopeSchema.safeParse(body);
		if (!parsedEnvelope.success) {
			logger.warn("Failed to parse Slack event envelope");
			return;
		}

		try {
			await handler.handle({
				slackEventId: parsedEnvelope.data.event_id,
				teamId: parsedEnvelope.data.team_id,
				trigger: "message",
				channel: event.channel,
				user: event.user,
				text: event.text,
				ts: event.ts,
				threadTs: event.thread_ts,
			});
		} catch (error) {
			logger.error("Failed to handle message event", {
				error: toErrorMessage(error),
			});
		}
	});

	app.event("app_mention", async ({ event, context, body }) => {
		if (!isProcessableMentionEvent(event)) {
			return;
		}
		if (context.botUserId === event.user) {
			return;
		}
		const parsedEnvelope = slackEventEnvelopeSchema.safeParse(body);
		if (!parsedEnvelope.success) {
			logger.warn("Failed to parse Slack app_mention envelope");
			return;
		}

		try {
			await handler.handle({
				slackEventId: parsedEnvelope.data.event_id,
				teamId: parsedEnvelope.data.team_id,
				trigger: "app_mention",
				channel: event.channel,
				user: event.user,
				text: event.text,
				ts: event.ts,
				threadTs: event.thread_ts,
			});
		} catch (error) {
			logger.error("Failed to handle app_mention event", {
				error: toErrorMessage(error),
			});
		}
	});
}
