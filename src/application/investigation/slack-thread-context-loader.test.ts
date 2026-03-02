import { describe, expect, it, vi } from "vitest";
import type {
	FetchSlackThreadHistoryInput,
	SlackThreadHistoryPort,
} from "../../ports/outbound/slack-thread-history";
import type { Logger } from "../../shared/observability/logger";
import type { SlackThreadMessage } from "../../shared/types/slack-thread-message";
import { SlackThreadContextLoader } from "./slack-thread-context-loader";

function createThreadHistoryPortMock(): SlackThreadHistoryPort & {
	fetchThreadHistory: ReturnType<
		typeof vi.fn<
			(input: FetchSlackThreadHistoryInput) => Promise<SlackThreadMessage[]>
		>
	>;
} {
	return {
		fetchThreadHistory: vi.fn(),
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

const BASE_LOG_META = {
	jobType: "alert_investigation",
	slackEventId: "Ev001",
	jobId: "job-1",
	channel: "C001",
	threadTs: "1710000000.000001",
	attempt: 1,
};

describe("SlackThreadContextLoader", () => {
	it("fetches thread history only for thread replies", async () => {
		const threadHistoryPort = createThreadHistoryPortMock();
		const logger = createLoggerMock();
		threadHistoryPort.fetchThreadHistory.mockResolvedValue([
			{
				ts: "1710000000.000001",
				user: "U001",
				text: "context",
			},
		]);
		const loader = new SlackThreadContextLoader({
			slackThreadHistoryPort: threadHistoryPort,
			logger,
		});

		const result = await loader.loadForMessage({
			message: {
				slackEventId: "Ev001",
				trigger: "app_mention",
				channel: "C001",
				user: "U001",
				text: "alert",
				ts: "1710000000.000002",
				threadTs: "1710000000.000001",
			},
			baseLogMeta: BASE_LOG_META,
		});

		expect(threadHistoryPort.fetchThreadHistory).toHaveBeenCalledWith({
			channel: "C001",
			threadTs: "1710000000.000001",
		});
		expect(result).toEqual([
			{
				ts: "1710000000.000001",
				user: "U001",
				text: "context",
			},
		]);
	});

	it("returns empty context for non-thread messages", async () => {
		const threadHistoryPort = createThreadHistoryPortMock();
		const logger = createLoggerMock();
		const loader = new SlackThreadContextLoader({
			slackThreadHistoryPort: threadHistoryPort,
			logger,
		});

		const result = await loader.loadForMessage({
			message: {
				slackEventId: "Ev001",
				trigger: "app_mention",
				channel: "C001",
				user: "U001",
				text: "alert",
				ts: "1710000000.000002",
			},
			baseLogMeta: BASE_LOG_META,
		});

		expect(threadHistoryPort.fetchThreadHistory).not.toHaveBeenCalled();
		expect(result).toEqual([]);
	});

	it("falls back with empty context when history fetch fails", async () => {
		const threadHistoryPort = createThreadHistoryPortMock();
		const logger = createLoggerMock();
		threadHistoryPort.fetchThreadHistory.mockRejectedValue(
			new Error("slack api failed"),
		);
		const loader = new SlackThreadContextLoader({
			slackThreadHistoryPort: threadHistoryPort,
			logger,
		});

		const result = await loader.loadForMessage({
			message: {
				slackEventId: "Ev001",
				trigger: "app_mention",
				channel: "C001",
				user: "U001",
				text: "alert",
				ts: "1710000000.000002",
				threadTs: "1710000000.000001",
			},
			baseLogMeta: BASE_LOG_META,
		});

		expect(result).toEqual([]);
		expect(logger.error).toHaveBeenCalledWith(
			"thread_context_fetch_failed",
			expect.objectContaining({
				jobId: "job-1",
			}),
		);
	});
});
