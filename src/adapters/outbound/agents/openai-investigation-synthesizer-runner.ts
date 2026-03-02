import { type Agent, RunContext, run } from "@openai/agents";
import { SYNTHESIZER_PROGRESS_OWNER_ID } from "../../../ports/outbound/investigation-progress-event";
import {
	type InvestigationSynthesizerRunnerPort,
	type RunSynthesizerInput,
	SynthesizerRunFailedError,
	type SynthesizerRunReport,
} from "../../../ports/outbound/investigation-synthesizer-runner";
import type { AlertContext } from "../../../shared/types/alert-context";
import type { InvestigationResult } from "../../../shared/types/investigation";
import { mapOpenAiUsageToLlmUsageSnapshot } from "./openai-llm-usage-mapper";
import { mapOpenAiRunStreamEventToProgressEvent } from "./openai-progress-event-mapper";

interface OpenAiInvestigationSynthesizerRunnerInput {
	createSynthesizerAgent: () => Agent;
}

export class OpenAiInvestigationSynthesizerRunner
	implements InvestigationSynthesizerRunnerPort
{
	constructor(
		private readonly input: OpenAiInvestigationSynthesizerRunnerInput,
	) {}

	async run(input: RunSynthesizerInput): Promise<SynthesizerRunReport> {
		const synthesizerInput = buildSynthesizerInput(
			input.result,
			input.alertContext,
		);
		const runContext = new RunContext();
		const synthesizerAgent = this.input.createSynthesizerAgent();

		try {
			const result = await run(synthesizerAgent, synthesizerInput, {
				maxTurns: 1,
				context: runContext,
				stream: true,
			});

			for await (const event of result) {
				const progressEvent = mapOpenAiRunStreamEventToProgressEvent(event);
				if (!progressEvent) {
					continue;
				}
				await input.onProgressEvent({
					ownerId: SYNTHESIZER_PROGRESS_OWNER_ID,
					event: progressEvent,
				});
			}

			return {
				reportText:
					result.finalOutput ||
					"Investigation completed but failed to generate a report.",
				usage: mapOpenAiUsageToLlmUsageSnapshot(runContext.usage),
			};
		} catch (error) {
			throw new SynthesizerRunFailedError({
				usage: mapOpenAiUsageToLlmUsageSnapshot(runContext.usage),
				cause: error,
			});
		}
	}
}

function buildSynthesizerInput(
	result: InvestigationResult,
	alertContext: AlertContext,
): string {
	const sections: string[] = [];

	sections.push("## Trigger Message");
	sections.push(alertContext.triggerMessageText);

	if (alertContext.threadTranscript.length > 0) {
		sections.push("\n## Thread Context");
		sections.push(alertContext.threadTranscript);
	}

	sections.push(`\n## Investigation Results`);
	sections.push(result.length > 0 ? result : "No investigation output.");

	return sections.join("\n");
}
