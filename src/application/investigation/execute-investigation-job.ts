import type {
	InvestigationContext,
	InvestigationResources,
	InvestigationRuntime,
} from "../../ports/outbound/investigation-context";
import type {
	CoordinatorRunReport,
	InvestigationCoordinatorRunnerPort,
} from "../../ports/outbound/investigation-coordinator-runner";
import {
	type InvestigationProgressEventCallback,
	SYNTHESIZER_PROGRESS_OWNER_ID,
} from "../../ports/outbound/investigation-progress-event";
import type {
	InvestigationSynthesizerRunnerPort,
	SynthesizerRunReport,
} from "../../ports/outbound/investigation-synthesizer-runner";
import type { SlackProgressStreamPort } from "../../ports/outbound/slack-progress-stream";
import type { SlackThreadHistoryPort } from "../../ports/outbound/slack-thread-history";
import type { SlackThreadReplyPort } from "../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../shared/observability/logger";
import type { AlertContext } from "../../shared/types/alert-context";
import type {
	InvestigationJobPayload,
	InvestigationJobType,
} from "../../shared/types/investigation-job";
import type { InvestigationLlmTelemetry } from "../../shared/types/investigation-llm-telemetry";
import { toErrorMessage } from "../../shared/utils/to-error-message";
import { extractAlertContext } from "../alert-intake/extract-alert-context";
import {
	createInvestigationExecutionFailedError,
	resolveInvestigationFailureError,
} from "./execution-errors";
import { buildInvestigationLlmTelemetry } from "./services/build-llm-telemetry";
import { CoordinatorProgressEventHandler } from "./services/coordinator-progress-event-handler";
import { createInvestigationProgressStreamSessionFactory } from "./services/investigation-progress-stream-session";
import { SlackThreadContextLoader } from "./slack-thread-context-loader";

const INVESTIGATION_TIMEOUT_MS = 1200 * 1000;

export interface InvestigationExecutionDeps {
	slackReplyPort: SlackThreadReplyPort;
	slackProgressStreamPort: SlackProgressStreamPort;
	slackThreadHistoryPort: SlackThreadHistoryPort;
	investigationResources: InvestigationResources;
	coordinatorRunner: InvestigationCoordinatorRunnerPort;
	synthesizerRunner: InvestigationSynthesizerRunnerPort;
	logger: Logger;
}

export interface ExecuteInvestigationJobInput {
	jobType: InvestigationJobType;
	jobId: string;
	retryCount: number;
	payload: InvestigationJobPayload;
	deps: InvestigationExecutionDeps;
}

export async function executeInvestigationJob(
	input: ExecuteInvestigationJobInput,
): Promise<void> {
	const threadTs = input.payload.message.threadTs ?? input.payload.message.ts;
	const startedAtMs = Date.now();
	const startedAtIso = new Date(startedAtMs).toISOString();
	const controller = new AbortController();
	const timeout = setTimeout(
		() => controller.abort(),
		INVESTIGATION_TIMEOUT_MS,
	);
	const baseLogMeta = {
		jobType: input.jobType,
		slackEventId: input.payload.slackEventId,
		jobId: input.jobId,
		channel: input.payload.message.channel,
		threadTs,
		attempt: input.retryCount + 1,
	};
	const progressSessionFactory =
		createInvestigationProgressStreamSessionFactory({
			slackStreamReplyPort: input.deps.slackProgressStreamPort,
			replyPort: input.deps.slackReplyPort,
			logger: input.deps.logger,
		});
	const progressSession = progressSessionFactory.createForThread({
		channel: input.payload.message.channel,
		threadTs,
		recipientUserId: input.payload.message.user,
		recipientTeamId: input.payload.message.teamId,
	});
	const progressEventHandler = new CoordinatorProgressEventHandler({
		progressSession,
	});
	const onProgressEvent: InvestigationProgressEventCallback = async (
		eventInput,
	) => {
		await progressEventHandler.handle(eventInput);
	};
	const threadContextLoader = new SlackThreadContextLoader({
		slackThreadHistoryPort: input.deps.slackThreadHistoryPort,
		logger: input.deps.logger,
	});

	try {
		const threadMessages = await threadContextLoader.loadForMessage({
			message: input.payload.message,
			baseLogMeta,
		});
		const alertContext = extractAlertContext({
			triggerMessageText: input.payload.message.text,
			threadMessages,
			botUserId: extractMentionedUserId(input.payload.message.text),
		});
		const runtime: InvestigationRuntime = {
			startedAtIso,
			channel: input.payload.message.channel,
			threadTs,
			retryCount: input.retryCount,
		};
		const context: InvestigationContext = {
			resources: input.deps.investigationResources,
			runtime,
		};
		await progressSession.start();

		const coordinatorReport = await input.deps.coordinatorRunner.run({
			alertContext,
			context,
			signal: controller.signal,
			onProgressEvent,
		});

		await progressSession.postReasoning({
			ownerId: SYNTHESIZER_PROGRESS_OWNER_ID,
			summaryText: "Reporting",
		});

		const synthesizerReport = await runSynthesisStage({
			coordinatorReport,
			alertContext,
			synthesizerRunner: input.deps.synthesizerRunner,
			onProgressEvent,
		});
		await progressSession.stopAsSucceeded();
		const llmTelemetry = buildInvestigationLlmTelemetry({
			coordinatorUsage: coordinatorReport.usage,
			synthesizerUsage: synthesizerReport.usage,
		});
		await postSlackReplyStage({
			slackReplyPort: input.deps.slackReplyPort,
			channel: input.payload.message.channel,
			threadTs,
			reportText: synthesizerReport.reportText,
			llmTelemetry,
		});
		const durationMs = Date.now() - startedAtMs;

		input.deps.logger.info("Processed investigation job", {
			...baseLogMeta,
			...buildLlmTokenLogMeta(llmTelemetry),
			worker_job_duration_ms: durationMs,
			latencyMs: durationMs,
		});
	} catch (error) {
		await progressSession.stopAsFailed();
		const failureError = resolveInvestigationFailureError(error);
		const llmTelemetry = buildInvestigationLlmTelemetry({
			coordinatorUsage: failureError.coordinatorUsage,
			synthesizerUsage: failureError.synthesizerUsage,
		});
		const durationMs = Date.now() - startedAtMs;
		input.deps.logger.error("Failed investigation job", {
			...baseLogMeta,
			...buildLlmTokenLogMeta(llmTelemetry),
			worker_job_duration_ms: durationMs,
			latencyMs: durationMs,
			error: toErrorMessage(failureError.error),
		});
		throw failureError.error;
	} finally {
		clearTimeout(timeout);
	}
}

function buildLlmTokenLogMeta(
	telemetry: InvestigationLlmTelemetry,
): Record<string, number> {
	return {
		llm_tokens_input_total: telemetry.total.inputTokens,
		llm_tokens_output_total: telemetry.total.outputTokens,
		llm_tokens_total: telemetry.total.totalTokens,
		llm_requests_total: telemetry.total.requests,
		llm_tokens_total_coordinator: telemetry.coordinator.totalTokens,
		llm_tokens_total_synthesizer: telemetry.synthesizer.totalTokens,
	};
}

async function runSynthesisStage(input: {
	coordinatorReport: CoordinatorRunReport;
	alertContext: AlertContext;
	synthesizerRunner: InvestigationSynthesizerRunnerPort;
	onProgressEvent: InvestigationProgressEventCallback;
}): Promise<SynthesizerRunReport> {
	try {
		return await input.synthesizerRunner.run({
			result: input.coordinatorReport.resultText,
			alertContext: input.alertContext,
			onProgressEvent: input.onProgressEvent,
		});
	} catch (error) {
		const failure = resolveInvestigationFailureError(error);
		throw createInvestigationExecutionFailedError({
			cause: failure.error,
			llmTelemetry: buildInvestigationLlmTelemetry({
				coordinatorUsage: input.coordinatorReport.usage,
				synthesizerUsage: failure.synthesizerUsage,
			}),
		});
	}
}

async function postSlackReplyStage(input: {
	slackReplyPort: SlackThreadReplyPort;
	channel: string;
	threadTs: string;
	reportText: string;
	llmTelemetry: InvestigationLlmTelemetry;
}): Promise<void> {
	try {
		await input.slackReplyPort.postThreadReply({
			channel: input.channel,
			threadTs: input.threadTs,
			text: input.reportText,
		});
	} catch (error) {
		throw createInvestigationExecutionFailedError({
			cause: error,
			llmTelemetry: input.llmTelemetry,
		});
	}
}

function extractMentionedUserId(text: string): string | undefined {
	const matchedMention = text.match(/<@([A-Z0-9]+)>/);
	return matchedMention?.[1];
}
