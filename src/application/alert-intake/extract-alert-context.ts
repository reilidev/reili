import type { AlertContext } from "../../shared/types/alert-context";
import type { SlackThreadMessage } from "../../shared/types/slack-thread-message";

export interface ExtractAlertContextInput {
	triggerMessageText: string;
	threadMessages: SlackThreadMessage[];
	botUserId?: string;
}

export function extractAlertContext(
	input: ExtractAlertContextInput,
): AlertContext {
	const triggerMessageText = input.triggerMessageText.trim();
	return {
		rawText: triggerMessageText,
		triggerMessageText,
		threadTranscript: buildThreadTranscript(
			input.threadMessages,
			input.botUserId,
		),
	};
}

function buildThreadTranscript(
	messages: SlackThreadMessage[],
	botUserId?: string,
): string {
	return messages
		.map((message) => {
			const author = normalizeAuthor(message.user, botUserId);
			const text = message.text.trim();
			const isoTimestamp = toIsoTimestamp(message.ts);
			return `[ts: ${message.ts} | iso: ${isoTimestamp}] ${author}: ${text}`;
		})
		.join("\n---\n");
}

function normalizeAuthor(user?: string, botUserId?: string): string {
	if (!user) {
		return "system";
	}

	const normalized = user.trim();
	if (normalized.length === 0) {
		return "system";
	}
	if (botUserId && normalized === botUserId) {
		return `${normalized} (You)`;
	}

	return normalized;
}

function toIsoTimestamp(ts: string): string {
	const [secondsPart, millisecondsPart = "0"] = ts.split(".");
	const seconds = Number(secondsPart);
	const milliseconds = Number(millisecondsPart.padEnd(3, "0").slice(0, 3));
	if (!Number.isFinite(seconds) || !Number.isFinite(milliseconds)) {
		return "unknown";
	}

	const dateFromTs = new Date(seconds * 1000 + milliseconds);
	const unixMillis = dateFromTs.getTime();
	if (Number.isNaN(unixMillis)) {
		return "unknown";
	}

	return dateFromTs.toISOString();
}
