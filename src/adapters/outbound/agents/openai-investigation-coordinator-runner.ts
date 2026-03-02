import { type Agent, RunContext, run } from "@openai/agents";
import type { InvestigationContext } from "../../../ports/outbound/investigation-context";
import {
	CoordinatorRunFailedError,
	type CoordinatorRunReport,
	type InvestigationCoordinatorRunnerPort,
	type RunCoordinatorInput,
} from "../../../ports/outbound/investigation-coordinator-runner";
import {
	COORDINATOR_PROGRESS_OWNER_ID,
	type InvestigationProgressEventCallback,
} from "../../../ports/outbound/investigation-progress-event";
import type { Logger } from "../../../shared/observability/logger";
import type { AlertContext } from "../../../shared/types/alert-context";
import { mapOpenAiUsageToLlmUsageSnapshot } from "./openai-llm-usage-mapper";
import { mapOpenAiRunStreamEventToProgressEvent } from "./openai-progress-event-mapper";
import {
	createOpenAiSubAgentStreamCallback,
	type OpenAiSubAgentStreamCallbackFactory,
} from "./openai-subagent-stream-callback";

const MAX_TURNS = 20;

interface OpenAiInvestigationCoordinatorRunnerInput {
	createCoordinatorAgent: (input: {
		onSubAgentStream?: OpenAiSubAgentStreamCallbackFactory;
	}) => Agent<InvestigationContext>;
	logger: Logger;
}

export class OpenAiInvestigationCoordinatorRunner
	implements InvestigationCoordinatorRunnerPort
{
	constructor(
		private readonly input: OpenAiInvestigationCoordinatorRunnerInput,
	) {}

	async run(input: RunCoordinatorInput): Promise<CoordinatorRunReport> {
		const coordinatorInput = buildCoordinatorInput(input.alertContext);
		const runContext = new RunContext<InvestigationContext>(input.context);
		const coordinatorAgent = this.input.createCoordinatorAgent({
			onSubAgentStream: this.createSubAgentCallbackFactory(
				input.onProgressEvent,
			),
		});

		try {
			const result = await run(coordinatorAgent, coordinatorInput, {
				context: runContext,
				signal: input.signal,
				maxTurns: MAX_TURNS,
				stream: true,
			});

			for await (const event of result) {
				const progressEvent = mapOpenAiRunStreamEventToProgressEvent(event);
				if (!progressEvent) {
					continue;
				}
				await input.onProgressEvent({
					ownerId: COORDINATOR_PROGRESS_OWNER_ID,
					event: progressEvent,
				});
			}

			return {
				resultText: result.finalOutput || "",
				usage: mapOpenAiUsageToLlmUsageSnapshot(runContext.usage),
			};
		} catch (error) {
			throw new CoordinatorRunFailedError({
				usage: mapOpenAiUsageToLlmUsageSnapshot(runContext.usage),
				cause: error,
			});
		}
	}

	private createSubAgentCallbackFactory(
		onProgressEvent: InvestigationProgressEventCallback,
	): OpenAiSubAgentStreamCallbackFactory {
		return (factoryInput) =>
			createOpenAiSubAgentStreamCallback({
				parentToolCallId: factoryInput.parentToolCallId,
				onProgressEvent,
				logger: this.input.logger,
			});
	}
}

function buildCoordinatorInput(alertContext: AlertContext): string {
	const investigationPrompt = `Investigate the following user input and respond with the most appropriate investigation or direct answer.
The input may be an alert, request, question, link, or partial context.`;
	const triggerMessageSection = `\n\nTrigger Message: ${alertContext.triggerMessageText}`;
	const threadContextSection =
		alertContext.threadTranscript.length > 0
			? `\n\nThread Context:\n${alertContext.threadTranscript}`
			: "";

	return `${investigationPrompt}${triggerMessageSection}${threadContextSection}`;
}
