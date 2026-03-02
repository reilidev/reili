import { describe, expect, it, vi } from "vitest";
import type { InvestigationResources } from "../../ports/outbound/investigation-context";
import type { InvestigationCoordinatorRunnerPort } from "../../ports/outbound/investigation-coordinator-runner";
import type { InvestigationSynthesizerRunnerPort } from "../../ports/outbound/investigation-synthesizer-runner";
import type {
	AppendSlackProgressStreamInput,
	SlackProgressStreamPort,
	StartSlackProgressStreamInput,
	StopSlackProgressStreamInput,
} from "../../ports/outbound/slack-progress-stream";
import type {
	FetchSlackThreadHistoryInput,
	SlackThreadHistoryPort,
} from "../../ports/outbound/slack-thread-history";
import type {
	SlackThreadReplyInput,
	SlackThreadReplyPort,
} from "../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../shared/observability/logger";
import type { InvestigationJobPayload } from "../../shared/types/investigation-job";
import type { LlmUsageSnapshot } from "../../shared/types/investigation-llm-telemetry";
import type { SlackThreadMessage } from "../../shared/types/slack-thread-message";
import {
	executeInvestigationJob,
	type InvestigationExecutionDeps,
} from "./execute-investigation-job";

const USAGE_SNAPSHOT: LlmUsageSnapshot = {
	requests: 1,
	inputTokens: 10,
	outputTokens: 20,
	totalTokens: 30,
};

function createSlackReplyPortMock(): SlackThreadReplyPort & {
	postThreadReply: ReturnType<
		typeof vi.fn<(input: SlackThreadReplyInput) => Promise<void>>
	>;
} {
	return {
		postThreadReply: vi.fn().mockResolvedValue(undefined),
	};
}

function createSlackProgressStreamPortMock(): SlackProgressStreamPort & {
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
		start: vi.fn().mockResolvedValue({ streamTs: "stream-1" }),
		append: vi.fn().mockResolvedValue(undefined),
		stop: vi.fn().mockResolvedValue(undefined),
	};
}

function createSlackThreadHistoryPortMock(): SlackThreadHistoryPort & {
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

function createCoordinatorRunnerMock(): InvestigationCoordinatorRunnerPort & {
	run: ReturnType<
		typeof vi.fn<
			(
				input: Parameters<InvestigationCoordinatorRunnerPort["run"]>[0],
			) => Promise<
				Awaited<ReturnType<InvestigationCoordinatorRunnerPort["run"]>>
			>
		>
	>;
} {
	return {
		run: vi.fn().mockResolvedValue({
			resultText: "coordinator result",
			usage: USAGE_SNAPSHOT,
		}),
	};
}

function createSynthesizerRunnerMock(): InvestigationSynthesizerRunnerPort & {
	run: ReturnType<
		typeof vi.fn<
			(
				input: Parameters<InvestigationSynthesizerRunnerPort["run"]>[0],
			) => Promise<
				Awaited<ReturnType<InvestigationSynthesizerRunnerPort["run"]>>
			>
		>
	>;
} {
	return {
		run: vi.fn().mockResolvedValue({
			reportText: "final report",
			usage: USAGE_SNAPSHOT,
		}),
	};
}

function createExecutionDeps(): {
	deps: InvestigationExecutionDeps;
	slackThreadHistoryPort: SlackThreadHistoryPort & {
		fetchThreadHistory: ReturnType<
			typeof vi.fn<
				(input: FetchSlackThreadHistoryInput) => Promise<SlackThreadMessage[]>
			>
		>;
	};
	coordinatorRunner: InvestigationCoordinatorRunnerPort & {
		run: ReturnType<
			typeof vi.fn<
				(
					input: Parameters<InvestigationCoordinatorRunnerPort["run"]>[0],
				) => Promise<
					Awaited<ReturnType<InvestigationCoordinatorRunnerPort["run"]>>
				>
			>
		>;
	};
	logger: Logger & {
		info: ReturnType<typeof vi.fn<(message: string) => void>>;
		warn: ReturnType<typeof vi.fn<(message: string) => void>>;
		error: ReturnType<typeof vi.fn<(message: string) => void>>;
	};
} {
	const slackReplyPort = createSlackReplyPortMock();
	const slackProgressStreamPort = createSlackProgressStreamPortMock();
	const slackThreadHistoryPort = createSlackThreadHistoryPortMock();
	const coordinatorRunner = createCoordinatorRunnerMock();
	const synthesizerRunner = createSynthesizerRunnerMock();
	const logger = createLoggerMock();
	const deps: InvestigationExecutionDeps = {
		slackReplyPort,
		slackProgressStreamPort,
		slackThreadHistoryPort,
		investigationResources: {} as InvestigationResources,
		coordinatorRunner,
		synthesizerRunner,
		logger,
	};

	return {
		deps,
		slackThreadHistoryPort,
		coordinatorRunner,
		logger,
	};
}

function createPayload(input: {
	ts: string;
	threadTs?: string;
	text?: string;
}): InvestigationJobPayload {
	return {
		slackEventId: "Ev001",
		message: {
			slackEventId: "Ev001",
			trigger: "app_mention",
			channel: "C001",
			user: "U001",
			teamId: "T001",
			text: input.text ?? "monitor alert",
			ts: input.ts,
			threadTs: input.threadTs,
		},
	};
}

describe("executeInvestigationJob", () => {
	it("fetches thread history only for thread replies", async () => {
		const { deps, slackThreadHistoryPort, coordinatorRunner } =
			createExecutionDeps();
		slackThreadHistoryPort.fetchThreadHistory.mockResolvedValue([
			{
				ts: "1710000000.000001",
				user: "U999",
				text: "thread context",
			},
		]);

		await executeInvestigationJob({
			jobType: "alert_investigation",
			jobId: "job-1",
			retryCount: 0,
			payload: createPayload({
				ts: "1710000000.000002",
				threadTs: "1710000000.000001",
				text: "<@U999> monitor alert",
			}),
			deps,
		});

		expect(slackThreadHistoryPort.fetchThreadHistory).toHaveBeenCalledWith({
			channel: "C001",
			threadTs: "1710000000.000001",
		});
		expect(coordinatorRunner.run).toHaveBeenCalledTimes(1);
		const runInput = coordinatorRunner.run.mock.calls[0]?.[0];
		expect(runInput?.alertContext.triggerMessageText).toBe(
			"<@U999> monitor alert",
		);
		expect(runInput?.alertContext.threadTranscript).toBe(
			"[ts: 1710000000.000001 | iso: 2024-03-09T16:00:00.000Z] U999 (You): thread context",
		);
	});

	it("does not fetch thread history for non-thread messages", async () => {
		const { deps, slackThreadHistoryPort, coordinatorRunner } =
			createExecutionDeps();

		await executeInvestigationJob({
			jobType: "alert_investigation",
			jobId: "job-2",
			retryCount: 0,
			payload: createPayload({
				ts: "1710000000.000100",
			}),
			deps,
		});

		expect(slackThreadHistoryPort.fetchThreadHistory).not.toHaveBeenCalled();
		expect(coordinatorRunner.run).toHaveBeenCalledTimes(1);
		const runInput = coordinatorRunner.run.mock.calls[0]?.[0];
		expect(runInput?.alertContext.threadTranscript).toBe("");
	});

	it("falls back when thread history fetch fails", async () => {
		const { deps, slackThreadHistoryPort, coordinatorRunner } =
			createExecutionDeps();
		slackThreadHistoryPort.fetchThreadHistory.mockRejectedValue(
			new Error("slack api failed"),
		);

		await executeInvestigationJob({
			jobType: "alert_investigation",
			jobId: "job-3",
			retryCount: 0,
			payload: createPayload({
				ts: "1710000000.000200",
				threadTs: "1710000000.000150",
			}),
			deps,
		});

		expect(coordinatorRunner.run).toHaveBeenCalledTimes(1);
		const runInput = coordinatorRunner.run.mock.calls[0]?.[0];
		expect(runInput?.alertContext.threadTranscript).toBe("");
	});
});
