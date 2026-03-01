import type { SlackMessage } from "./slack-message";

export type InvestigationJobType = "alert_investigation";

export interface InvestigationJobPayload {
	slackEventId: string;
	message: SlackMessage;
}

interface InvestigationJobBase<
	TJobType extends InvestigationJobType,
	TPayload extends InvestigationJobPayload,
> {
	jobId: string;
	jobType: TJobType;
	receivedAt: string;
	payload: TPayload;
	retryCount: number;
}

export type AlertInvestigationJob = InvestigationJobBase<
	"alert_investigation",
	InvestigationJobPayload
>;

export type InvestigationJob = AlertInvestigationJob;
