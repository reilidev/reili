export const COORDINATOR_PROGRESS_OWNER_ID = "coordinator";
export const SYNTHESIZER_PROGRESS_OWNER_ID = "synthesizer";

export interface ReasoningSummaryCreatedEvent {
	type: "reasoning_summary_created";
	summaryText: string;
}

export interface ToolCallStartedEvent {
	type: "tool_call_started";
	taskId: string;
	title: string;
}

export interface ToolCallCompletedEvent {
	type: "tool_call_completed";
	taskId: string;
	title: string;
}

export interface MessageOutputCreatedEvent {
	type: "message_output_created";
}

export type InvestigationProgressEvent =
	| ReasoningSummaryCreatedEvent
	| ToolCallStartedEvent
	| ToolCallCompletedEvent
	| MessageOutputCreatedEvent;

export interface InvestigationProgressEventCallbackInput {
	ownerId: string;
	event: InvestigationProgressEvent;
}

export type InvestigationProgressEventCallback = (
	input: InvestigationProgressEventCallbackInput,
) => Promise<void>;
