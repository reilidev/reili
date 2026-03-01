import { describe, expect, it, vi } from "vitest";
import type {
	AppendSlackProgressStreamInput,
	SlackProgressStreamPort,
	StartSlackProgressStreamInput,
	StopSlackProgressStreamInput,
} from "../../../ports/outbound/slack-progress-stream";
import type {
	SlackThreadReplyInput,
	SlackThreadReplyPort,
} from "../../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../../shared/observability/logger";
import { createInvestigationProgressStreamSessionFactory } from "./investigation-progress-stream-session";

function createStreamPortMock(): SlackProgressStreamPort & {
	start: ReturnType<
		typeof vi.fn<
			(input: StartSlackProgressStreamInput) => Promise<{ streamTs: string }>
		>
	>;
	append: ReturnType<
		typeof vi.fn<(input: AppendSlackProgressStreamInput) => Promise<void>>
	>;
	stop: ReturnType<
		typeof vi.fn<(input: StopSlackProgressStreamInput) => Promise<void>>
	>;
} {
	return {
		start: vi.fn(),
		append: vi.fn(),
		stop: vi.fn(),
	};
}

function createReplyPortMock(): SlackThreadReplyPort & {
	postThreadReply: ReturnType<
		typeof vi.fn<(input: SlackThreadReplyInput) => Promise<void>>
	>;
} {
	return {
		postThreadReply: vi.fn(),
	};
}

function createLoggerMock(): Logger & {
	info: ReturnType<typeof vi.fn<(message: string) => void>>;
	warn: ReturnType<typeof vi.fn<(message: string) => void>>;
	error: ReturnType<typeof vi.fn<(message: string) => void>>;
} {
	return {
		info: vi.fn(),
		warn: vi.fn(),
		error: vi.fn(),
	};
}

const CHANNEL = "C123";
const THREAD_TS = "123.456";
const RECIPIENT_USER_ID = "U123";

describe("createInvestigationProgressStreamSessionFactory", () => {
	it("restarts stream when append fails with message_not_in_streaming_state", async () => {
		const streamPort = createStreamPortMock();
		streamPort.start
			.mockResolvedValueOnce({ streamTs: "stream-initial" })
			.mockResolvedValueOnce({ streamTs: "stream-restarted" });
		streamPort.append.mockRejectedValueOnce(
			new Error("Error: An API error occurred: message_not_in_streaming_state"),
		);
		streamPort.stop.mockResolvedValue(undefined);

		const replyPort = createReplyPortMock();
		const logger = createLoggerMock();
		const session = createInvestigationProgressStreamSessionFactory({
			slackStreamReplyPort: streamPort,
			replyPort,
			logger,
		}).createForThread({
			channel: CHANNEL,
			threadTs: THREAD_TS,
			recipientUserId: RECIPIENT_USER_ID,
		});

		await session.start();
		await session.postReasoning({
			ownerId: "coordinator",
			summaryText: "Collect evidence\nInspect logs",
		});
		await session.stopAsSucceeded();

		expect(streamPort.start).toHaveBeenCalledTimes(2);
		expect(streamPort.append).toHaveBeenCalledTimes(1);
		expect(streamPort.stop).toHaveBeenCalledTimes(1);
		expect(streamPort.stop.mock.calls[0]?.[0].streamTs).toBe(
			"stream-restarted",
		);
		expect(replyPort.postThreadReply).toHaveBeenCalledTimes(0);
		expect(
			logger.info.mock.calls.some(
				([message]) => message === "slack_stream_restarted",
			),
		).toBe(true);
		expect(streamPort.start.mock.calls[1]?.[0].chunks).toMatchObject([
			{
				type: "task_update",
				title: "Collect evidence",
				status: "in_progress",
				details: "Inspect logs\n",
			},
		]);
	});

	it("falls back to thread reply when stream restart fails", async () => {
		const streamPort = createStreamPortMock();
		streamPort.start
			.mockResolvedValueOnce({ streamTs: "stream-initial" })
			.mockRejectedValueOnce(new Error("failed to restart stream"));
		streamPort.append.mockRejectedValueOnce(
			new Error("Error: An API error occurred: message_not_in_streaming_state"),
		);

		const replyPort = createReplyPortMock();
		replyPort.postThreadReply.mockResolvedValue(undefined);

		const logger = createLoggerMock();
		const session = createInvestigationProgressStreamSessionFactory({
			slackStreamReplyPort: streamPort,
			replyPort,
			logger,
		}).createForThread({
			channel: CHANNEL,
			threadTs: THREAD_TS,
			recipientUserId: RECIPIENT_USER_ID,
		});

		await session.start();
		await session.postReasoning({
			ownerId: "coordinator",
			summaryText: "Collect evidence",
		});
		await session.stopAsSucceeded();

		expect(streamPort.start).toHaveBeenCalledTimes(2);
		expect(streamPort.stop).toHaveBeenCalledTimes(0);
		expect(replyPort.postThreadReply).toHaveBeenCalledWith({
			channel: CHANNEL,
			threadTs: THREAD_TS,
			text: ":hammer_and_wrench: Collect evidence",
		});
		expect(
			logger.warn.mock.calls.some(
				([message]) => message === "slack_stream_fallback_mode",
			),
		).toBe(true);
	});
});
