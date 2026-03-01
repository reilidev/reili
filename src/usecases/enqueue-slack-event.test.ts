import { describe, expect, it, vi } from "vitest";
import type {
	SlackThreadReplyInput,
	SlackThreadReplyPort,
} from "../ports/outbound/slack-thread-reply";
import type { WorkerJobDispatcherPort } from "../ports/outbound/worker-job-dispatcher";
import type { Logger } from "../shared/observability/logger";
import type { InvestigationJob } from "../shared/types/investigation-job";
import type { SlackMessage } from "../shared/types/slack-message";
import {
	EnqueueSlackEventUseCase,
	type EnqueueSlackEventUseCaseDeps,
} from "./enqueue-slack-event";

function createWorkerJobDispatcherMock(): WorkerJobDispatcherPort & {
	dispatch: ReturnType<typeof vi.fn<(job: InvestigationJob) => Promise<void>>>;
} {
	return {
		dispatch: vi.fn().mockResolvedValue(undefined),
	};
}

function createSlackReplyPortMock(): SlackThreadReplyPort & {
	postThreadReply: ReturnType<
		typeof vi.fn<(input: SlackThreadReplyInput) => Promise<void>>
	>;
} {
	return {
		postThreadReply: vi.fn().mockResolvedValue(undefined),
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

function createUseCase(): {
	useCase: EnqueueSlackEventUseCase;
	workerJobDispatcher: WorkerJobDispatcherPort & {
		dispatch: ReturnType<
			typeof vi.fn<(job: InvestigationJob) => Promise<void>>
		>;
	};
} {
	const workerJobDispatcher = createWorkerJobDispatcherMock();
	const slackReplyPort = createSlackReplyPortMock();
	const logger = createLoggerMock();
	const deps: EnqueueSlackEventUseCaseDeps = {
		workerJobDispatcher,
		slackReplyPort,
		logger,
		jobMaxRetry: 0,
		jobBackoffMs: 0,
	};

	return {
		useCase: new EnqueueSlackEventUseCase(deps),
		workerJobDispatcher,
	};
}

function createMessage(input: { text: string }): SlackMessage {
	return {
		slackEventId: "Ev001",
		trigger: "message",
		channel: "C001",
		user: "U001",
		text: input.text,
		ts: "1710000000.000001",
	};
}

describe("EnqueueSlackEventUseCase", () => {
	it("dispatches alert_investigation job for normal alerts", async () => {
		const { useCase, workerJobDispatcher } = createUseCase();

		await useCase.handle(createMessage({ text: "high latency detected" }));

		expect(workerJobDispatcher.dispatch).toHaveBeenCalledTimes(1);
		expect(workerJobDispatcher.dispatch).toHaveBeenCalledWith(
			expect.objectContaining({
				jobType: "alert_investigation",
			}),
		);
	});

	it("dispatches alert_investigation job for security alerts", async () => {
		const { useCase, workerJobDispatcher } = createUseCase();

		await useCase.handle(createMessage({ text: "security incident detected" }));

		const dispatchedJob = workerJobDispatcher.dispatch.mock.calls[0]?.[0];
		expect(dispatchedJob).toBeDefined();
		if (!dispatchedJob) {
			return;
		}

		expect(dispatchedJob).toMatchObject({
			jobType: "alert_investigation",
		});
		expect("securityCategory" in dispatchedJob.payload).toBe(false);
	});
});
