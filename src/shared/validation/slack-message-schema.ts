import { z } from "zod";
import type { SlackMessage, SlackTriggerType } from "../types/slack-message";

export const slackTriggerTypeSchema: z.ZodType<SlackTriggerType> = z.enum([
	"message",
	"app_mention",
]);

export const slackMessageSchema: z.ZodType<SlackMessage> = z.object({
	slackEventId: z.string().min(1),
	teamId: z.string().min(1).optional(),
	trigger: slackTriggerTypeSchema,
	channel: z.string().min(1),
	user: z.string().min(1),
	text: z.string(),
	ts: z.string().min(1),
	threadTs: z.string().min(1).optional(),
});
