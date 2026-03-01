import type { SlackThreadReplyPort } from "../../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../../shared/observability/logger";
import { toErrorMessage } from "../../../shared/utils/to-error-message";

const REASONING_SUMMARY_MAX_LENGTH = 200;

export interface CreateInvestigationProgressNotifierInput {
	slackReplyPort: SlackThreadReplyPort;
	channel: string;
	threadTs: string;
	logger: Logger;
}

export interface InvestigationProgressNotifier {
	postReasoning(summaryText: string): void;
}

export function createInvestigationProgressNotifier(
	input: CreateInvestigationProgressNotifierInput,
): InvestigationProgressNotifier {
	return new SlackInvestigationProgressNotifier(input);
}

class SlackInvestigationProgressNotifier
	implements InvestigationProgressNotifier
{
	constructor(
		private readonly input: CreateInvestigationProgressNotifierInput,
	) {}

	postReasoning(summaryText: string): void {
		const formattedSummary = formatReasoningSummary(summaryText);
		if (!formattedSummary) {
			return;
		}

		this.postProgressMessage(`:thought_balloon: ${formattedSummary}`);
	}

	private postProgressMessage(text: string): void {
		void this.input.slackReplyPort
			.postThreadReply({
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				text,
			})
			.catch((error) => {
				this.input.logger.warn("Failed to post investigation progress", {
					channel: this.input.channel,
					threadTs: this.input.threadTs,
					error: toErrorMessage(error),
				});
			});
	}
}

function formatReasoningSummary(summaryText: string): string | undefined {
	const trimmedSummary = summaryText.trim();
	if (trimmedSummary.length === 0) {
		return undefined;
	}

	if (trimmedSummary.length <= REASONING_SUMMARY_MAX_LENGTH) {
		return trimmedSummary;
	}

	const clippedSummary = trimmedSummary
		.slice(0, REASONING_SUMMARY_MAX_LENGTH)
		.trimEnd();

	return `${clippedSummary}...`;
}
