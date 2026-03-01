import type { SlackThreadHistoryPort } from "../../ports/outbound/slack-thread-history";
import type { Logger } from "../../shared/observability/logger";
import type { SlackMessage } from "../../shared/types/slack-message";
import type { SlackThreadMessage } from "../../shared/types/slack-thread-message";
import { toErrorMessage } from "../../shared/utils/to-error-message";

interface SlackThreadContextLoaderDeps {
	slackThreadHistoryPort: SlackThreadHistoryPort;
	logger: Logger;
}

export interface SlackThreadContextLoaderInput {
	message: SlackMessage;
	baseLogMeta: Record<string, string | number>;
}

export class SlackThreadContextLoader {
	constructor(private readonly deps: SlackThreadContextLoaderDeps) {}

	async loadForMessage(
		input: SlackThreadContextLoaderInput,
	): Promise<SlackThreadMessage[]> {
		if (!isThreadReplyMessage(input.message)) {
			return [];
		}

		const threadTs = input.message.threadTs ?? input.message.ts;
		const startedAtMs = Date.now();
		try {
			return await this.deps.slackThreadHistoryPort.fetchThreadHistory({
				channel: input.message.channel,
				threadTs,
			});
		} catch (error) {
			this.deps.logger.error("thread_context_fetch_failed", {
				...input.baseLogMeta,
				thread_context_fetch_latency_ms: Date.now() - startedAtMs,
				error: toErrorMessage(error),
			});
			return [];
		}
	}
}

function isThreadReplyMessage(message: SlackMessage): boolean {
	const threadTs = message.threadTs;
	if (!threadTs) {
		return false;
	}

	return threadTs !== message.ts;
}
