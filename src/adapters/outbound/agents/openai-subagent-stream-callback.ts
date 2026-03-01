import type { FunctionCallItem, RunStreamEvent } from "@openai/agents";
import type {
	InvestigationProgressEvent,
	InvestigationProgressEventCallback,
} from "../../../ports/outbound/investigation-progress-event";
import type { Logger } from "../../../shared/observability/logger";
import { toErrorMessage } from "../../../shared/utils/to-error-message";
import { mapOpenAiRunStreamEventToProgressEvent } from "./openai-progress-event-mapper";

interface OpenAiSubAgentStreamEvent {
	event: RunStreamEvent;
	toolCall?: FunctionCallItem;
}

export type OpenAiSubAgentStreamCallback = (
	streamEvent: OpenAiSubAgentStreamEvent,
) => Promise<void>;

export type OpenAiSubAgentStreamCallbackFactory = (input: {
	parentToolCallId: string;
}) => OpenAiSubAgentStreamCallback;

interface CreateOpenAiSubAgentStreamCallbackInput {
	parentToolCallId: string;
	onProgressEvent: InvestigationProgressEventCallback;
	logger: Logger;
}

export function createOpenAiSubAgentStreamCallback(
	input: CreateOpenAiSubAgentStreamCallbackInput,
): OpenAiSubAgentStreamCallback {
	let chain = Promise.resolve();

	return (streamEvent: OpenAiSubAgentStreamEvent): Promise<void> => {
		chain = chain
			.then(async () => {
				const ownerId = resolveOwnerId({
					parentToolCallId: input.parentToolCallId,
					streamEvent,
				});
				if (!ownerId) {
					input.logger.warn("subagent_progress_event_owner_not_found", {
						parentToolCallId: input.parentToolCallId,
						eventType: streamEvent.event.type,
						eventName:
							streamEvent.event.type === "run_item_stream_event"
								? streamEvent.event.name
								: undefined,
					});
					return;
				}

				const progressEvent = mapOpenAiRunStreamEventToProgressEvent(
					streamEvent.event,
				);
				if (!progressEvent) {
					return;
				}

				await postProgressEvent({
					onProgressEvent: input.onProgressEvent,
					ownerId,
					progressEvent,
				});
			})
			.catch((error) => {
				input.logger.warn("subagent_progress_event_failed", {
					parentToolCallId: input.parentToolCallId,
					error: toErrorMessage(error),
				});
			});
		return chain;
	};
}

async function postProgressEvent(input: {
	onProgressEvent: InvestigationProgressEventCallback;
	ownerId: string;
	progressEvent: InvestigationProgressEvent;
}): Promise<void> {
	await input.onProgressEvent({
		ownerId: input.ownerId,
		event: input.progressEvent,
	});
}

function resolveOwnerId(input: {
	parentToolCallId: string;
	streamEvent: OpenAiSubAgentStreamEvent;
}): string | undefined {
	if (input.streamEvent.toolCall?.callId) {
		return `${input.parentToolCallId}:${input.streamEvent.toolCall.callId}`;
	}
	return undefined;
}
