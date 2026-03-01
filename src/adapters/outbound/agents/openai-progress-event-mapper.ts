import type { RunItem, RunStreamEvent } from "@openai/agents";
import type {
	InvestigationProgressEvent,
	ToolCallCompletedEvent,
	ToolCallStartedEvent,
} from "../../../ports/outbound/investigation-progress-event";

export function mapOpenAiRunStreamEventToProgressEvent(
	event: RunStreamEvent,
): InvestigationProgressEvent | undefined {
	if (event.type !== "run_item_stream_event") {
		return undefined;
	}

	if (event.name === "reasoning_item_created") {
		const summaryText = extractReasoningSummaryText(event.item);
		if (!summaryText) {
			return undefined;
		}
		return {
			type: "reasoning_summary_created",
			summaryText,
		};
	}

	if (event.name === "tool_called") {
		return extractToolStartedEvent(event.item);
	}

	if (event.name === "tool_output") {
		return extractToolCompletedEvent(event.item);
	}

	if (event.name === "message_output_created") {
		return {
			type: "message_output_created",
		};
	}

	return undefined;
}

function extractReasoningSummaryText(item: RunItem): string | undefined {
	if (item.type !== "reasoning_item") {
		return undefined;
	}

	const summaryText = item.rawItem.content
		.map((part) => part.text.replace(/\*\*/g, "").trim())
		.filter((text) => text.length > 0)
		.join(" ");

	if (summaryText.length === 0) {
		return undefined;
	}

	return summaryText;
}

function extractToolStartedEvent(
	item: RunItem,
): ToolCallStartedEvent | undefined {
	if (item.type !== "tool_call_item") {
		return undefined;
	}

	const metadata = extractToolCallMetadata(item.rawItem);
	if (!metadata) {
		return undefined;
	}

	return {
		type: "tool_call_started",
		taskId: metadata.callId,
		title: metadata.toolName,
	};
}

function extractToolCompletedEvent(
	item: RunItem,
): ToolCallCompletedEvent | undefined {
	if (item.type !== "tool_call_output_item") {
		return undefined;
	}

	const metadata = extractToolOutputMetadata(item.rawItem);
	if (!metadata) {
		return undefined;
	}

	return {
		type: "tool_call_completed",
		taskId: metadata.callId,
		title: metadata.title,
	};
}

interface ToolCallMetadata {
	callId: string;
	toolName: string;
}

interface ToolOutputMetadata {
	callId: string;
	title: string;
}

function extractToolCallMetadata(
	rawItem: RunItem["rawItem"],
): ToolCallMetadata | undefined {
	if (!rawItem) {
		return undefined;
	}

	if (rawItem.type === "function_call") {
		return {
			callId: rawItem.callId,
			toolName: rawItem.name,
		};
	}

	if (rawItem.type === "hosted_tool_call") {
		if (!rawItem.id) {
			return undefined;
		}

		return {
			callId: rawItem.id,
			toolName: rawItem.name,
		};
	}

	if (rawItem.type === "computer_call") {
		return {
			callId: rawItem.callId,
			toolName: "computer",
		};
	}

	if (rawItem.type === "shell_call") {
		return {
			callId: rawItem.callId,
			toolName: "shell",
		};
	}

	if (rawItem.type === "apply_patch_call") {
		return {
			callId: rawItem.callId,
			toolName: "apply_patch",
		};
	}

	return undefined;
}

function extractToolOutputMetadata(
	rawItem: RunItem["rawItem"],
): ToolOutputMetadata | undefined {
	if (!rawItem) {
		return undefined;
	}

	if (rawItem.type === "function_call_result") {
		return {
			callId: rawItem.callId,
			title: rawItem.name,
		};
	}

	if (rawItem.type === "computer_call_result") {
		return {
			callId: rawItem.callId,
			title: "computer",
		};
	}

	if (rawItem.type === "shell_call_output") {
		return {
			callId: rawItem.callId,
			title: "shell",
		};
	}

	if (rawItem.type === "apply_patch_call_output") {
		return {
			callId: rawItem.callId,
			title: "apply_patch",
		};
	}

	return undefined;
}
