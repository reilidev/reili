import type { InvestigationProgressEventCallbackInput } from "../../../ports/outbound/investigation-progress-event";
import type {
	InvestigationProgressMessageOutputCreatedInput,
	InvestigationProgressReasoningInput,
	InvestigationProgressStreamSession,
	InvestigationProgressTaskUpdateInput,
} from "./investigation-progress-stream-session";

interface CoordinatorProgressEventHandlerInput {
	progressSession: InvestigationProgressStreamSession;
}

export class CoordinatorProgressEventHandler {
	private readonly progressSession: InvestigationProgressStreamSession;

	constructor(input: CoordinatorProgressEventHandlerInput) {
		this.progressSession = input.progressSession;
	}

	async handle(input: InvestigationProgressEventCallbackInput): Promise<void> {
		if (input.event.type === "reasoning_summary_created") {
			await this.postReasoning({
				ownerId: input.ownerId,
				summaryText: input.event.summaryText,
			});
			return;
		}

		if (input.event.type === "tool_call_started") {
			await this.postToolStarted({
				ownerId: input.ownerId,
				taskId: input.event.taskId,
				title: input.event.title,
			});
			return;
		}

		if (input.event.type === "tool_call_completed") {
			await this.postToolCompleted({
				ownerId: input.ownerId,
				taskId: input.event.taskId,
				title: input.event.title,
			});
			return;
		}

		if (input.event.type === "message_output_created") {
			await this.postMessageOutputCreated({
				ownerId: input.ownerId,
			});
		}
	}

	private async postReasoning(
		input: InvestigationProgressReasoningInput,
	): Promise<void> {
		if (input.summaryText.trim().length === 0) {
			return;
		}
		await this.progressSession.postReasoning(input);
	}

	private async postToolStarted(
		input: InvestigationProgressTaskUpdateInput,
	): Promise<void> {
		await this.progressSession.postToolStarted(input);
	}

	private async postToolCompleted(
		input: InvestigationProgressTaskUpdateInput,
	): Promise<void> {
		await this.progressSession.postToolCompleted(input);
	}

	private async postMessageOutputCreated(
		input: InvestigationProgressMessageOutputCreatedInput,
	): Promise<void> {
		await this.progressSession.postMessageOutputCreated(input);
	}
}
