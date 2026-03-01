import { randomUUID } from "node:crypto";
import type {
	SlackAnyChunk,
	SlackProgressStreamPort,
} from "../../../ports/outbound/slack-progress-stream";
import type { SlackThreadReplyPort } from "../../../ports/outbound/slack-thread-reply";
import type { Logger } from "../../../shared/observability/logger";
import { toErrorMessage } from "../../../shared/utils/to-error-message";

const STREAM_START_TEXT = ":hourglass_flowing_sand:";

export interface CreateInvestigationProgressStreamSessionFactoryInput {
	slackStreamReplyPort: SlackProgressStreamPort;
	replyPort: SlackThreadReplyPort;
	logger: Logger;
}

export interface InvestigationProgressTaskUpdateInput {
	ownerId: string;
	taskId: string;
	title: string;
}

export interface InvestigationProgressReasoningInput {
	ownerId: string;
	summaryText: string;
}

export interface InvestigationProgressMessageOutputCreatedInput {
	ownerId: string;
}

export interface CreateInvestigationProgressStreamSessionInput {
	channel: string;
	threadTs: string;
	recipientUserId: string;
	recipientTeamId?: string;
}

export interface InvestigationProgressStreamSession {
	start(): Promise<void>;
	postReasoning(input: InvestigationProgressReasoningInput): Promise<void>;
	postToolStarted(input: InvestigationProgressTaskUpdateInput): Promise<void>;
	postToolCompleted(input: InvestigationProgressTaskUpdateInput): Promise<void>;
	postMessageOutputCreated(
		input: InvestigationProgressMessageOutputCreatedInput,
	): Promise<void>;
	stopAsSucceeded(): Promise<void>;
	stopAsFailed(): Promise<void>;
}

export interface InvestigationProgressStreamSessionFactory {
	createForThread(
		input: CreateInvestigationProgressStreamSessionInput,
	): InvestigationProgressStreamSession;
}

export function createInvestigationProgressStreamSessionFactory(
	input: CreateInvestigationProgressStreamSessionFactoryInput,
): InvestigationProgressStreamSessionFactory {
	return new SlackInvestigationProgressStreamSessionFactory(input);
}

class SlackInvestigationProgressStreamSessionFactory
	implements InvestigationProgressStreamSessionFactory
{
	constructor(
		private readonly input: CreateInvestigationProgressStreamSessionFactoryInput,
	) {}

	createForThread(
		input: CreateInvestigationProgressStreamSessionInput,
	): InvestigationProgressStreamSession {
		return new SlackInvestigationProgressStreamSession({
			slackStreamReplyPort: this.input.slackStreamReplyPort,
			replyPort: this.input.replyPort,
			logger: this.input.logger,
			channel: input.channel,
			threadTs: input.threadTs,
			recipientUserId: input.recipientUserId,
			recipientTeamId: input.recipientTeamId,
		});
	}
}

interface CreateSlackInvestigationProgressStreamSessionInput
	extends CreateInvestigationProgressStreamSessionFactoryInput,
		CreateInvestigationProgressStreamSessionInput {}

class SlackInvestigationProgressStreamSession
	implements InvestigationProgressStreamSession
{
	private streamTs?: string;
	private streamStopped = false;
	private fallbackMode = false;
	private appendCount = 0;
	private lastErrorMessage?: string;
	private readonly activeReasoningScopeIdByOwnerId = new Map<string, string>();
	private readonly reasoningScopeById = new Map<string, ReasoningScope>();
	private readonly reasoningScopeIdByTaskId = new Map<string, string>();
	private readonly completedReasoningScopeIdsByOwnerId = new Map<
		string,
		Set<string>
	>();
	private readonly latestCompletedReasoningScopeIdByOwnerId = new Map<
		string,
		string
	>();

	constructor(
		private readonly input: CreateSlackInvestigationProgressStreamSessionInput,
	) {}

	async start(): Promise<void> {
		if (this.streamStopped || this.streamTs) {
			return;
		}

		try {
			const stream = await this.input.slackStreamReplyPort.start({
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				recipientUserId: this.input.recipientUserId,
				recipientTeamId: this.input.recipientTeamId,
				chunks: [
					{
						type: "markdown_text",
						text: STREAM_START_TEXT,
					},
				],
			});
			this.streamTs = stream.streamTs;
			this.input.logger.info("slack_stream_started", {
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				streamTs: this.streamTs,
				slack_stream_fallback_mode: this.fallbackMode,
			});
		} catch (error) {
			const errorMessage = toErrorMessage(error);
			await this.enableFallbackMode({
				errorMessage,
				reason: "start_failed",
			});
			await this.postFallbackMessage(STREAM_START_TEXT);
		}
	}

	async postReasoning(
		input: InvestigationProgressReasoningInput,
	): Promise<void> {
		const formattedSummary = parseReasoningSummary(input.summaryText);
		if (!formattedSummary) {
			return;
		}
		await this.completeActiveReasoningScopeIfIdle({
			ownerId: input.ownerId,
		});

		const scope = this.createReasoningScope({
			ownerId: input.ownerId,
			title: formattedSummary.title,
		});
		this.activeReasoningScopeIdByOwnerId.set(input.ownerId, scope.scopeId);
		await this.appendReasoningScopeUpdate({
			scope,
			status: "in_progress",
			detailLine: formattedSummary.details,
		});
	}

	async postToolStarted(
		input: InvestigationProgressTaskUpdateInput,
	): Promise<void> {
		const scope = this.resolveScopeForToolStarted(input);
		this.upsertScopeToolStatus({
			scope,
			ownerId: input.ownerId,
			taskId: input.taskId,
			status: "in_progress",
		});
		this.resolveCompletedReasoningScopeIdsByOwnerId(input.ownerId).delete(
			scope.scopeId,
		);
		await this.appendReasoningScopeUpdate({
			scope,
			status: "in_progress",
			detailLine: buildToolDetailLine({
				toolName: input.title,
			}),
		});
	}

	async postToolCompleted(
		input: InvestigationProgressTaskUpdateInput,
	): Promise<void> {
		const scope = this.resolveScopeForToolCompleted(input);
		if (!scope) {
			return;
		}
		this.upsertScopeToolStatus({
			scope,
			ownerId: input.ownerId,
			taskId: input.taskId,
			status: "complete",
		});
		if (resolveReasoningScopeStatus(scope) === "complete") {
			return;
		}
		await this.appendReasoningScopeUpdate({
			scope,
			status: "in_progress",
		});
	}

	async postMessageOutputCreated(
		input: InvestigationProgressMessageOutputCreatedInput,
	): Promise<void> {
		for (const scope of this.reasoningScopeById.values()) {
			if (scope.ownerId !== input.ownerId) {
				continue;
			}
			await this.completeScopeIfNeeded(scope);
		}
		this.activeReasoningScopeIdByOwnerId.delete(input.ownerId);
	}

	private resolveScopeForToolStarted(
		input: InvestigationProgressTaskUpdateInput,
	): ReasoningScope {
		const taskOwnershipKey = buildTaskOwnershipKey({
			ownerId: input.ownerId,
			taskId: input.taskId,
		});
		const existingScopeId = this.reasoningScopeIdByTaskId.get(taskOwnershipKey);
		if (existingScopeId) {
			const existingScope = this.reasoningScopeById.get(existingScopeId);
			if (existingScope) {
				return existingScope;
			}
		}

		const activeReasoningScopeId = this.activeReasoningScopeIdByOwnerId.get(
			input.ownerId,
		);
		if (activeReasoningScopeId) {
			const activeScope = this.reasoningScopeById.get(activeReasoningScopeId);
			if (activeScope) {
				this.reasoningScopeIdByTaskId.set(
					taskOwnershipKey,
					activeScope.scopeId,
				);
				return activeScope;
			}
			this.activeReasoningScopeIdByOwnerId.delete(input.ownerId);
		}

		return this.reopenScopeForToolStarted(input);
	}

	private resolveScopeForToolCompleted(
		input: InvestigationProgressTaskUpdateInput,
	): ReasoningScope | undefined {
		const taskOwnershipKey = buildTaskOwnershipKey({
			ownerId: input.ownerId,
			taskId: input.taskId,
		});
		const existingScopeId = this.reasoningScopeIdByTaskId.get(taskOwnershipKey);
		if (existingScopeId) {
			const existingScope = this.reasoningScopeById.get(existingScopeId);
			if (existingScope) {
				return existingScope;
			}
		}

		this.input.logger.warn("reasoning_scope_not_found_for_tool_completed", {
			channel: this.input.channel,
			threadTs: this.input.threadTs,
			ownerId: input.ownerId,
			taskId: input.taskId,
			toolName: input.title,
		});
		return undefined;
	}

	private createReasoningScope(input: {
		ownerId: string;
		title: string;
	}): ReasoningScope {
		const scopeId = this.createReasoningScopeId();
		const scope: ReasoningScope = {
			scopeId,
			ownerId: input.ownerId,
			title: input.title,
			toolStatusByTaskId: new Map<string, ReasoningScopeToolStatus>(),
		};
		this.reasoningScopeById.set(scope.scopeId, scope);
		return scope;
	}

	private createReasoningScopeId(): string {
		let scopeId = `reasoning-${randomUUID()}`;
		while (this.reasoningScopeById.has(scopeId)) {
			scopeId = `reasoning-${randomUUID()}`;
		}
		return scopeId;
	}

	private upsertScopeToolStatus(input: {
		scope: ReasoningScope;
		ownerId: string;
		taskId: string;
		status: ReasoningScopeToolStatus;
	}): void {
		input.scope.toolStatusByTaskId.set(input.taskId, input.status);
		this.reasoningScopeIdByTaskId.set(
			buildTaskOwnershipKey({
				ownerId: input.ownerId,
				taskId: input.taskId,
			}),
			input.scope.scopeId,
		);
	}

	private async completeActiveReasoningScopeIfIdle(input: {
		ownerId: string;
	}): Promise<void> {
		const activeReasoningScopeId = this.activeReasoningScopeIdByOwnerId.get(
			input.ownerId,
		);
		if (!activeReasoningScopeId) {
			return;
		}

		const activeScope = this.reasoningScopeById.get(activeReasoningScopeId);
		if (!activeScope || scopeHasInProgressTool(activeScope)) {
			if (!activeScope) {
				this.activeReasoningScopeIdByOwnerId.delete(input.ownerId);
			}
			return;
		}

		await this.completeScopeIfNeeded(activeScope);
	}

	private async completeScopeIfNeeded(scope: ReasoningScope): Promise<void> {
		const completedScopeIds = this.resolveCompletedReasoningScopeIdsByOwnerId(
			scope.ownerId,
		);
		if (completedScopeIds.has(scope.scopeId)) {
			return;
		}
		completedScopeIds.add(scope.scopeId);
		this.latestCompletedReasoningScopeIdByOwnerId.set(
			scope.ownerId,
			scope.scopeId,
		);
		await this.appendReasoningScopeUpdate({
			scope,
			status: "complete",
		});
	}

	private reopenScopeForToolStarted(
		input: InvestigationProgressTaskUpdateInput,
	): ReasoningScope {
		const lastCompletedScope = this.resolveLatestCompletedScopeByOwnerId(
			input.ownerId,
		);
		const reopenedScope = this.createReasoningScope({
			ownerId: input.ownerId,
			title: lastCompletedScope?.title ?? "Tool executions",
		});
		this.activeReasoningScopeIdByOwnerId.set(
			input.ownerId,
			reopenedScope.scopeId,
		);
		this.input.logger.info("reasoning_scope_reopened_for_tool_started", {
			channel: this.input.channel,
			threadTs: this.input.threadTs,
			ownerId: input.ownerId,
			taskId: input.taskId,
			toolName: input.title,
			reopenedFromScopeId: lastCompletedScope?.scopeId,
			reopenedScopeId: reopenedScope.scopeId,
		});
		return reopenedScope;
	}

	private resolveLatestCompletedScopeByOwnerId(
		ownerId: string,
	): ReasoningScope | undefined {
		const latestCompletedScopeId =
			this.latestCompletedReasoningScopeIdByOwnerId.get(ownerId);
		if (!latestCompletedScopeId) {
			return undefined;
		}

		const latestCompletedScope = this.reasoningScopeById.get(
			latestCompletedScopeId,
		);
		if (latestCompletedScope) {
			return latestCompletedScope;
		}
		this.latestCompletedReasoningScopeIdByOwnerId.delete(ownerId);
		return undefined;
	}

	private resolveCompletedReasoningScopeIdsByOwnerId(
		ownerId: string,
	): Set<string> {
		const completedScopeIds =
			this.completedReasoningScopeIdsByOwnerId.get(ownerId);
		if (completedScopeIds) {
			return completedScopeIds;
		}

		const nextCompletedScopeIds = new Set<string>();
		this.completedReasoningScopeIdsByOwnerId.set(
			ownerId,
			nextCompletedScopeIds,
		);
		return nextCompletedScopeIds;
	}

	private async appendReasoningScopeUpdate(input: {
		scope: ReasoningScope;
		status: ReasoningScopeStatus;
		detailLine?: string;
	}): Promise<void> {
		const chunk: SlackAnyChunk = {
			type: "task_update",
			id: input.scope.scopeId,
			title: input.scope.title,
			status: input.status,
			details: input.detailLine,
			output: input.status === "complete" ? "done" : undefined,
		};
		await this.append({
			chunks: [chunk],
			fallbackText: buildReasoningScopeFallbackText({
				scope: input.scope,
				status: input.status,
				detailLine: input.detailLine,
			}),
		});
	}

	async stopAsSucceeded(): Promise<void> {
		await this.stop();
	}

	async stopAsFailed(): Promise<void> {
		await this.stop();
	}

	private async stop(): Promise<void> {
		if (this.streamStopped) {
			return;
		}

		if (this.fallbackMode || !this.streamTs) {
			this.streamStopped = true;
			this.logStop();
			return;
		}

		try {
			await this.input.slackStreamReplyPort.stop({
				channel: this.input.channel,
				streamTs: this.streamTs,
			});
		} catch (error) {
			this.lastErrorMessage = toErrorMessage(error);
			this.input.logger.warn("Failed to stop Slack progress stream", {
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				streamTs: this.streamTs,
				error: this.lastErrorMessage,
				slack_stream_last_error: this.lastErrorMessage,
			});
		} finally {
			this.streamStopped = true;
			this.logStop();
		}
	}

	private async append(input: {
		chunks: SlackAnyChunk[];
		fallbackText: string;
	}): Promise<void> {
		if (this.streamStopped) {
			return;
		}

		if (this.fallbackMode || !this.streamTs) {
			await this.postFallbackMessage(input.fallbackText);
			return;
		}

		try {
			await this.input.slackStreamReplyPort.append({
				channel: this.input.channel,
				streamTs: this.streamTs,
				chunks: input.chunks,
			});
			this.appendCount += 1;
		} catch (error) {
			const errorMessage = toErrorMessage(error);
			const failedStreamTs = this.streamTs;
			this.lastErrorMessage = errorMessage;
			this.input.logger.warn("Failed to append Slack progress stream", {
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				streamTs: failedStreamTs,
				error: errorMessage,
				slack_stream_last_error: errorMessage,
			});

			if (isMessageNotInStreamingStateError(errorMessage)) {
				await this.restartStreamWithChunks({
					chunks: input.chunks,
					fallbackText: input.fallbackText,
					failedStreamTs,
					errorMessage,
				});
				return;
			}

			if (isPermanentStreamAppendError(errorMessage)) {
				await this.enableFallbackMode({
					errorMessage,
					reason: "append_failed_permanent",
				});
				await this.postFallbackMessage(input.fallbackText);
			}
		}
	}

	private async restartStreamWithChunks(input: {
		chunks: SlackAnyChunk[];
		fallbackText: string;
		failedStreamTs?: string;
		errorMessage: string;
	}): Promise<void> {
		try {
			const stream = await this.input.slackStreamReplyPort.start({
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				recipientUserId: this.input.recipientUserId,
				recipientTeamId: this.input.recipientTeamId,
				chunks: input.chunks,
			});
			this.streamTs = stream.streamTs;
			this.appendCount += 1;
			this.input.logger.info("slack_stream_restarted", {
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				previousStreamTs: input.failedStreamTs,
				streamTs: this.streamTs,
				error: input.errorMessage,
				slack_stream_last_error: input.errorMessage,
			});
		} catch (error) {
			const restartErrorMessage = toErrorMessage(error);
			await this.enableFallbackMode({
				errorMessage: restartErrorMessage,
				reason: "append_failed_stream_restart_failed",
			});
			await this.postFallbackMessage(input.fallbackText);
		}
	}

	private async enableFallbackMode(input: {
		errorMessage: string;
		reason: string;
	}): Promise<void> {
		this.fallbackMode = true;
		this.lastErrorMessage = input.errorMessage;
		this.input.logger.warn("slack_stream_fallback_mode", {
			channel: this.input.channel,
			threadTs: this.input.threadTs,
			streamTs: this.streamTs,
			reason: input.reason,
			error: this.lastErrorMessage,
			slack_stream_fallback_mode: this.fallbackMode,
			slack_stream_last_error: this.lastErrorMessage,
		});
	}

	private async postFallbackMessage(text: string): Promise<void> {
		try {
			await this.input.replyPort.postThreadReply({
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				text,
			});
		} catch (error) {
			this.input.logger.warn("Failed to post fallback progress message", {
				channel: this.input.channel,
				threadTs: this.input.threadTs,
				error: toErrorMessage(error),
			});
		}
	}

	private logStop(): void {
		this.input.logger.info("slack_stream_stopped", {
			channel: this.input.channel,
			threadTs: this.input.threadTs,
			streamTs: this.streamTs,
			slack_stream_append_count: this.appendCount,
			slack_stream_fallback_mode: this.fallbackMode,
			slack_stream_last_error: this.lastErrorMessage,
		});
	}
}

type ReasoningScopeStatus = "in_progress" | "complete";
type ReasoningScopeToolStatus = ReasoningScopeStatus;

interface ReasoningScope {
	scopeId: string;
	ownerId: string;
	title: string;
	toolStatusByTaskId: Map<string, ReasoningScopeToolStatus>;
}

function buildTaskOwnershipKey(input: {
	ownerId: string;
	taskId: string;
}): string {
	return `${input.ownerId}:${input.taskId}`;
}

function scopeHasInProgressTool(scope: ReasoningScope): boolean {
	for (const toolStatus of scope.toolStatusByTaskId.values()) {
		if (toolStatus === "in_progress") {
			return true;
		}
	}
	return false;
}

function resolveReasoningScopeStatus(
	scope: ReasoningScope,
): ReasoningScopeStatus {
	if (scope.toolStatusByTaskId.size === 0) {
		return "in_progress";
	}

	for (const status of scope.toolStatusByTaskId.values()) {
		if (status === "in_progress") {
			return "in_progress";
		}
	}

	return "complete";
}

function buildReasoningScopeFallbackText(input: {
	scope: ReasoningScope;
	status: ReasoningScopeStatus;
	detailLine?: string;
}): string {
	const detailsText = input.detailLine ? `\n${input.detailLine}` : "";
	if (input.status === "complete") {
		return `:white_check_mark: ${input.scope.title} が完了しました${detailsText}`;
	}

	return `:hammer_and_wrench: ${input.scope.title}${detailsText}`;
}

function buildToolDetailLine(input: { toolName: string }): string {
	return `${input.toolName}\n`;
}

interface ParsedReasoningSummary {
	title: string;
	details?: string;
}

function parseReasoningSummary(
	summaryText: string,
): ParsedReasoningSummary | undefined {
	const trimmedSummary = summaryText.trim();
	if (trimmedSummary.length === 0) {
		return undefined;
	}

	const lines = trimmedSummary
		.split("\n")
		.map((line) => line.trim())
		.filter((line) => line.length > 0);
	if (lines.length === 0) {
		return undefined;
	}

	const title = lines[0];
	const details = lines.length >= 2 ? `${lines[1]}\n` : undefined;

	return {
		title,
		details,
	};
}

function isPermanentStreamAppendError(errorMessage: string): boolean {
	const lowerMessage = errorMessage.toLowerCase();
	return (
		lowerMessage.includes("invalid_ts") ||
		lowerMessage.includes("message_not_found") ||
		lowerMessage.includes("channel_not_found")
	);
}

function isMessageNotInStreamingStateError(errorMessage: string): boolean {
	return errorMessage.toLowerCase().includes("message_not_in_streaming_state");
}
