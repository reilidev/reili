import { z } from "zod";
import type { InvestigationJob } from "../types/investigation-job";
import { slackMessageSchema } from "./slack-message-schema";

const baseJobSchema = z.object({
	jobId: z.string().min(1),
	receivedAt: z.string().datetime(),
	retryCount: z.number().int().min(0),
});

const investigationJobPayloadSchema = z.object({
	slackEventId: z.string().min(1),
	message: slackMessageSchema,
});

const alertInvestigationJobSchema = baseJobSchema.extend({
	jobType: z.literal("alert_investigation"),
	payload: investigationJobPayloadSchema,
});

export const investigationJobSchema: z.ZodType<InvestigationJob> =
	alertInvestigationJobSchema;
