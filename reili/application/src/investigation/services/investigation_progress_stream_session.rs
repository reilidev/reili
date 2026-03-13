use std::sync::Arc;

use async_trait::async_trait;
use reili_shared::ports::outbound::slack_progress_stream::{
    SlackChunkSourceType, SlackMarkdownTextChunk, SlackTaskUpdateChunk, SlackTaskUpdateStatus,
};
use reili_shared::ports::outbound::{
    AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort,
    StartSlackProgressStreamInput, StopSlackProgressStreamInput,
};
use serde_json::Value;

use crate::investigation::logger::{InvestigationLogger, string_log_meta};

use super::progress_stream_state::{
    ProgressStep, ProgressStepStatus, ProgressStreamState, ResolveToolStartedProgressStepOutput,
    ToolCallStatus, resolve_progress_step_status,
};

const STREAM_START_TEXT: &str = ":hourglass_flowing_sand:";
const STREAM_ROTATION_CHARACTER_LIMIT: usize = 2800;

pub struct CreateInvestigationProgressStreamSessionFactoryInput {
    pub slack_stream_reply_port: Arc<dyn SlackProgressStreamPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationProgressTaskUpdateInput {
    pub owner_id: String,
    pub task_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationProgressReasoningInput {
    pub owner_id: String,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationProgressMessageOutputCreatedInput {
    pub owner_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInvestigationProgressStreamSessionInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
}

#[async_trait]
pub trait InvestigationProgressStreamSession: Send {
    async fn start(&mut self);
    async fn post_reasoning(&mut self, input: InvestigationProgressReasoningInput);
    async fn post_tool_started(&mut self, input: InvestigationProgressTaskUpdateInput);
    async fn post_tool_completed(&mut self, input: InvestigationProgressTaskUpdateInput);
    async fn post_message_output_created(
        &mut self,
        input: InvestigationProgressMessageOutputCreatedInput,
    );
    async fn stop_as_succeeded(&mut self);
    async fn stop_as_failed(&mut self);
}

pub trait InvestigationProgressStreamSessionFactory: Send + Sync {
    fn create_for_thread(
        &self,
        input: CreateInvestigationProgressStreamSessionInput,
    ) -> Box<dyn InvestigationProgressStreamSession>;
}

#[must_use]
pub fn create_investigation_progress_stream_session_factory(
    input: CreateInvestigationProgressStreamSessionFactoryInput,
) -> impl InvestigationProgressStreamSessionFactory {
    SlackInvestigationProgressStreamSessionFactory { input }
}

struct SlackInvestigationProgressStreamSessionFactory {
    input: CreateInvestigationProgressStreamSessionFactoryInput,
}

impl InvestigationProgressStreamSessionFactory for SlackInvestigationProgressStreamSessionFactory {
    fn create_for_thread(
        &self,
        input: CreateInvestigationProgressStreamSessionInput,
    ) -> Box<dyn InvestigationProgressStreamSession> {
        Box::new(SlackInvestigationProgressStreamSession::new(
            CreateSlackInvestigationProgressStreamSessionInput {
                slack_stream_reply_port: Arc::clone(&self.input.slack_stream_reply_port),
                logger: Arc::clone(&self.input.logger),
                channel: input.channel,
                thread_ts: input.thread_ts,
                recipient_user_id: input.recipient_user_id,
                recipient_team_id: input.recipient_team_id,
            },
        ))
    }
}

struct CreateSlackInvestigationProgressStreamSessionInput {
    slack_stream_reply_port: Arc<dyn SlackProgressStreamPort>,
    logger: Arc<dyn InvestigationLogger>,
    channel: String,
    thread_ts: String,
    recipient_user_id: String,
    recipient_team_id: Option<String>,
}

struct SlackInvestigationProgressStreamSession {
    input: CreateSlackInvestigationProgressStreamSessionInput,
    stream_ts: Option<String>,
    current_stream_character_count: usize,
    stream_stopped: bool,
    append_count: u64,
    last_error_message: Option<String>,
    state: ProgressStreamState,
}

struct RecoverFromAppendFailureInput {
    chunks: Vec<SlackAnyChunk>,
    failed_stream_ts: String,
    error_message: String,
}

impl SlackInvestigationProgressStreamSession {
    fn new(input: CreateSlackInvestigationProgressStreamSessionInput) -> Self {
        Self {
            input,
            stream_ts: None,
            current_stream_character_count: 0,
            stream_stopped: false,
            append_count: 0,
            last_error_message: None,
            state: ProgressStreamState::new(),
        }
    }

    async fn append_progress_step_update(
        &mut self,
        progress_step: &ProgressStep,
        status: ProgressStepStatus,
        detail_line: Option<String>,
    ) {
        let chunk = SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: progress_step.progress_step_id.clone(),
            title: progress_step.title.clone(),
            status: to_slack_task_status(&status),
            details: detail_line.clone(),
            output: if status == ProgressStepStatus::Complete {
                Some("done".to_string())
            } else {
                None
            },
            sources: None,
        });

        self.append(vec![chunk]).await;
    }

    async fn complete_active_progress_step_if_idle(&mut self, owner_id: &str) {
        let Some(progress_step) = self.state.complete_active_progress_step_if_idle(owner_id) else {
            return;
        };

        self.append_progress_step_update(&progress_step, ProgressStepStatus::Complete, None)
            .await;
    }

    async fn complete_progress_step_if_needed(&mut self, progress_step_id: &str) {
        let Some(progress_step) = self.state.mark_progress_step_completed(progress_step_id) else {
            return;
        };

        self.append_progress_step_update(&progress_step, ProgressStepStatus::Complete, None)
            .await;
    }

    fn log_reopened_progress_step(
        &self,
        input: &InvestigationProgressTaskUpdateInput,
        output: &ResolveToolStartedProgressStepOutput,
    ) {
        let Some(reopened_from_progress_step_id) = output.reopened_from_progress_step_id.clone()
        else {
            return;
        };

        self.input.logger.info(
            "progress_step_reopened_for_tool_started",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("ownerId", input.owner_id.clone()),
                ("taskId", input.task_id.clone()),
                ("toolName", input.title.clone()),
                ("reopenedProgressStepId", output.progress_step_id.clone()),
                ("reopenedFromProgressStepId", reopened_from_progress_step_id),
            ]),
        );
    }

    fn log_missing_progress_step_for_tool_completed(
        &self,
        input: &InvestigationProgressTaskUpdateInput,
    ) {
        self.input.logger.warn(
            "progress_step_not_found_for_tool_completed",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("ownerId", input.owner_id.clone()),
                ("taskId", input.task_id.clone()),
                ("toolName", input.title.clone()),
            ]),
        );
    }

    async fn stop(&mut self) {
        if self.stream_stopped {
            return;
        }

        if self.stream_ts.is_none() {
            self.stream_stopped = true;
            self.log_stop();
            return;
        }

        let stream_ts = self.stream_ts.clone().unwrap_or_default();
        let stop_result = self
            .input
            .slack_stream_reply_port
            .stop(StopSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                stream_ts: stream_ts.clone(),
                markdown_text: None,
                chunks: None,
                blocks: None,
            })
            .await;
        if let Err(error) = stop_result {
            self.last_error_message = Some(error.message.clone());
            self.input.logger.warn(
                "Failed to stop Slack progress stream",
                string_log_meta([
                    ("channel", self.input.channel.clone()),
                    ("threadTs", self.input.thread_ts.clone()),
                    ("streamTs", stream_ts),
                    ("error", error.message.clone()),
                    ("slack_stream_last_error", error.message),
                ]),
            );
        }

        self.stream_stopped = true;
        self.log_stop();
    }

    async fn append(&mut self, chunks: Vec<SlackAnyChunk>) {
        if self.stream_stopped {
            return;
        }

        if self.stream_ts.is_none() {
            return;
        }

        if self.should_rotate_stream(&chunks) {
            self.rotate_stream_with_chunks(chunks).await;
            return;
        }

        let stream_ts = self.stream_ts.clone().unwrap_or_default();
        let chunk_character_count = count_chunk_characters(&chunks);
        let append_result = self
            .input
            .slack_stream_reply_port
            .append(AppendSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                stream_ts: stream_ts.clone(),
                markdown_text: None,
                chunks: Some(chunks.clone()),
            })
            .await;

        if append_result.is_ok() {
            self.append_count = self.append_count.saturating_add(1);
            self.current_stream_character_count = self
                .current_stream_character_count
                .saturating_add(chunk_character_count);
            return;
        }

        self.recover_from_append_failure(RecoverFromAppendFailureInput {
            chunks,
            failed_stream_ts: stream_ts,
            error_message: append_result
                .expect_err("append_result should be error")
                .message,
        })
        .await;
    }

    async fn recover_from_append_failure(&mut self, input: RecoverFromAppendFailureInput) {
        self.last_error_message = Some(input.error_message.clone());
        self.input.logger.warn(
            "Failed to append Slack progress stream",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("streamTs", input.failed_stream_ts.clone()),
                ("error", input.error_message.clone()),
                ("slack_stream_last_error", input.error_message.clone()),
            ]),
        );

        if is_message_not_in_streaming_state_error(&input.error_message) {
            self.restart_stream_with_chunks(
                input.chunks,
                Some(input.failed_stream_ts),
                input.error_message,
            )
            .await;
            return;
        }

        if is_message_too_long_error(&input.error_message) {
            self.rotate_stream_with_chunks(input.chunks).await;
            return;
        }

        if is_permanent_stream_append_error(&input.error_message) {
            self.disable_stream(input.error_message, "append_failed_permanent");
        }
    }

    fn should_rotate_stream(&self, chunks: &[SlackAnyChunk]) -> bool {
        self.current_stream_character_count
            .saturating_add(count_chunk_characters(chunks))
            > STREAM_ROTATION_CHARACTER_LIMIT
    }

    async fn restart_stream_with_chunks(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
        failed_stream_ts: Option<String>,
        error_message: String,
    ) {
        let chunk_character_count = count_chunk_characters(&chunks);
        let start_result = self
            .input
            .slack_stream_reply_port
            .start(StartSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                thread_ts: self.input.thread_ts.clone(),
                recipient_user_id: self.input.recipient_user_id.clone(),
                recipient_team_id: self.input.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(chunks),
            })
            .await;

        let Ok(stream) = start_result else {
            let restart_error_message = start_result
                .expect_err("start_result should be error")
                .message;
            self.disable_stream(restart_error_message, "append_failed_stream_restart_failed");
            return;
        };

        self.stream_ts = Some(stream.stream_ts.clone());
        self.current_stream_character_count = chunk_character_count;
        self.append_count = self.append_count.saturating_add(1);

        let mut meta = string_log_meta([
            ("channel", self.input.channel.clone()),
            ("threadTs", self.input.thread_ts.clone()),
            ("streamTs", stream.stream_ts),
            ("error", error_message.clone()),
            ("slack_stream_last_error", error_message),
        ]);
        if let Some(previous_stream_ts) = failed_stream_ts {
            meta.insert(
                "previousStreamTs".to_string(),
                Value::String(previous_stream_ts),
            );
        }
        self.input.logger.info("slack_stream_restarted", meta);
    }

    async fn rotate_stream_with_chunks(&mut self, chunks: Vec<SlackAnyChunk>) {
        let previous_stream_ts = self.stream_ts.clone();
        let previous_character_count = self.current_stream_character_count;
        let chunk_character_count = count_chunk_characters(&chunks);

        self.stop_current_stream_for_rotation().await;

        let start_result = self
            .input
            .slack_stream_reply_port
            .start(StartSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                thread_ts: self.input.thread_ts.clone(),
                recipient_user_id: self.input.recipient_user_id.clone(),
                recipient_team_id: self.input.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(chunks),
            })
            .await;

        let Ok(stream) = start_result else {
            let error_message = start_result
                .expect_err("start_result should be error")
                .message;
            self.disable_stream(error_message, "stream_rotation_start_failed");
            return;
        };

        self.stream_ts = Some(stream.stream_ts.clone());
        self.current_stream_character_count = chunk_character_count;
        self.append_count = self.append_count.saturating_add(1);

        let mut meta = string_log_meta([
            ("channel", self.input.channel.clone()),
            ("threadTs", self.input.thread_ts.clone()),
            ("streamTs", stream.stream_ts),
            (
                "slack_stream_character_count",
                self.current_stream_character_count.to_string(),
            ),
            (
                "slack_stream_character_limit",
                STREAM_ROTATION_CHARACTER_LIMIT.to_string(),
            ),
            (
                "previousSlackStreamCharacterCount",
                previous_character_count.to_string(),
            ),
        ]);
        if let Some(previous_stream_ts) = previous_stream_ts {
            meta.insert(
                "previousStreamTs".to_string(),
                Value::String(previous_stream_ts),
            );
        }
        self.input.logger.info("slack_stream_rotated", meta);
    }

    async fn stop_current_stream_for_rotation(&mut self) {
        let Some(stream_ts) = self.stream_ts.clone() else {
            return;
        };

        let stop_result = self
            .input
            .slack_stream_reply_port
            .stop(StopSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                stream_ts: stream_ts.clone(),
                markdown_text: None,
                chunks: None,
                blocks: None,
            })
            .await;

        if let Err(error) = stop_result {
            self.last_error_message = Some(error.message.clone());
            self.input.logger.warn(
                "Failed to stop Slack progress stream before rotation",
                string_log_meta([
                    ("channel", self.input.channel.clone()),
                    ("threadTs", self.input.thread_ts.clone()),
                    ("streamTs", stream_ts),
                    ("error", error.message.clone()),
                    ("slack_stream_last_error", error.message),
                ]),
            );
        }
    }

    fn disable_stream(&mut self, error_message: String, reason: &str) {
        self.stream_ts = None;
        self.current_stream_character_count = 0;
        self.last_error_message = Some(error_message.clone());

        self.input.logger.warn(
            "slack_progress_stream_disabled",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("reason", reason.to_string()),
                ("error", error_message.clone()),
                ("slack_stream_last_error", error_message),
            ]),
        );
    }

    fn log_start_failure(&mut self, error_message: String) {
        self.last_error_message = Some(error_message.clone());
        self.input.logger.warn(
            "Failed to start Slack progress stream",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("error", error_message.clone()),
                ("slack_stream_last_error", error_message),
            ]),
        );
    }

    fn log_stop(&self) {
        let mut meta = string_log_meta([
            ("channel", self.input.channel.clone()),
            ("threadTs", self.input.thread_ts.clone()),
            ("slack_stream_append_count", self.append_count.to_string()),
        ]);
        if let Some(stream_ts) = &self.stream_ts {
            meta.insert("streamTs".to_string(), Value::String(stream_ts.clone()));
        }
        if let Some(last_error_message) = &self.last_error_message {
            meta.insert(
                "slack_stream_last_error".to_string(),
                Value::String(last_error_message.clone()),
            );
        }

        self.input.logger.info("slack_stream_stopped", meta);
    }
}

#[async_trait]
impl InvestigationProgressStreamSession for SlackInvestigationProgressStreamSession {
    async fn start(&mut self) {
        if self.stream_stopped || self.stream_ts.is_some() {
            return;
        }

        let start_result = self
            .input
            .slack_stream_reply_port
            .start(StartSlackProgressStreamInput {
                channel: self.input.channel.clone(),
                thread_ts: self.input.thread_ts.clone(),
                recipient_user_id: self.input.recipient_user_id.clone(),
                recipient_team_id: self.input.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
                    text: STREAM_START_TEXT.to_string(),
                })]),
            })
            .await;

        match start_result {
            Ok(stream) => {
                self.stream_ts = Some(stream.stream_ts.clone());
                self.current_stream_character_count =
                    count_chunk_characters(&[SlackAnyChunk::MarkdownText(
                        SlackMarkdownTextChunk {
                            text: STREAM_START_TEXT.to_string(),
                        },
                    )]);
                let mut meta = string_log_meta([
                    ("channel", self.input.channel.clone()),
                    ("threadTs", self.input.thread_ts.clone()),
                    ("streamTs", stream.stream_ts),
                ]);
                if let Some(last_error_message) = &self.last_error_message {
                    meta.insert(
                        "slack_stream_last_error".to_string(),
                        Value::String(last_error_message.clone()),
                    );
                }
                self.input.logger.info("slack_stream_started", meta);
            }
            Err(error) => {
                self.log_start_failure(error.message);
            }
        }
    }

    async fn post_reasoning(&mut self, input: InvestigationProgressReasoningInput) {
        if input.title.trim().is_empty() {
            return;
        }

        self.complete_active_progress_step_if_idle(&input.owner_id)
            .await;

        let progress_step_id = self
            .state
            .create_progress_step(&input.owner_id, &input.title);
        self.state
            .set_active_progress_step(&input.owner_id, progress_step_id.clone());

        if let Some(progress_step) = self.state.progress_step(&progress_step_id) {
            self.append_progress_step_update(
                &progress_step,
                ProgressStepStatus::InProgress,
                normalize_reasoning_summary(&input.summary),
            )
            .await;
        }
    }

    async fn post_tool_started(&mut self, input: InvestigationProgressTaskUpdateInput) {
        let resolved = self.state.resolve_progress_step_for_tool_started(
            &input.owner_id,
            &input.task_id,
            "Tool executions",
        );
        self.log_reopened_progress_step(&input, &resolved);
        self.state.upsert_progress_step_tool_call_status(
            &resolved.progress_step_id,
            &input.owner_id,
            &input.task_id,
            ToolCallStatus::InProgress,
        );

        self.state
            .mark_progress_step_incomplete(&input.owner_id, &resolved.progress_step_id);

        if let Some(progress_step) = self.state.progress_step(&resolved.progress_step_id) {
            self.append_progress_step_update(
                &progress_step,
                ProgressStepStatus::InProgress,
                Some(build_tool_detail_line(&input.title)),
            )
            .await;
        }
    }

    async fn post_tool_completed(&mut self, input: InvestigationProgressTaskUpdateInput) {
        let Some(progress_step_id) = self
            .state
            .resolve_progress_step_for_tool_completed(&input.owner_id, &input.task_id)
        else {
            self.log_missing_progress_step_for_tool_completed(&input);
            return;
        };

        self.state.upsert_progress_step_tool_call_status(
            &progress_step_id,
            &input.owner_id,
            &input.task_id,
            ToolCallStatus::Complete,
        );

        let Some(progress_step) = self.state.progress_step(&progress_step_id) else {
            return;
        };

        if resolve_progress_step_status(&progress_step) == ProgressStepStatus::Complete {
            return;
        }

        self.append_progress_step_update(&progress_step, ProgressStepStatus::InProgress, None)
            .await;
    }

    async fn post_message_output_created(
        &mut self,
        input: InvestigationProgressMessageOutputCreatedInput,
    ) {
        for progress_step_id in self.state.progress_step_ids_for_owner(&input.owner_id) {
            self.complete_progress_step_if_needed(&progress_step_id)
                .await;
        }

        self.state.clear_active_progress_step(&input.owner_id);
    }

    async fn stop_as_succeeded(&mut self) {
        self.stop().await;
    }

    async fn stop_as_failed(&mut self) {
        self.stop().await;
    }
}

fn to_slack_task_status(status: &ProgressStepStatus) -> SlackTaskUpdateStatus {
    match status {
        ProgressStepStatus::InProgress => SlackTaskUpdateStatus::InProgress,
        ProgressStepStatus::Complete => SlackTaskUpdateStatus::Complete,
    }
}

fn build_tool_detail_line(tool_name: &str) -> String {
    format!("{tool_name}\n")
}

fn normalize_reasoning_summary(summary: &str) -> Option<String> {
    let trimmed_summary = summary.trim();
    if trimmed_summary.is_empty() {
        return None;
    }

    Some(format!("{trimmed_summary}\n"))
}

fn is_permanent_stream_append_error(error_message: &str) -> bool {
    let lower_message = error_message.to_lowercase();
    lower_message.contains("invalid_ts")
        || lower_message.contains("message_not_found")
        || lower_message.contains("channel_not_found")
        || lower_message.contains("invalid_arguments")
}

fn is_message_too_long_error(error_message: &str) -> bool {
    error_message.to_lowercase().contains("msg_too_long")
}

fn is_message_not_in_streaming_state_error(error_message: &str) -> bool {
    error_message
        .to_lowercase()
        .contains("message_not_in_streaming_state")
}

fn count_chunk_characters(chunks: &[SlackAnyChunk]) -> usize {
    chunks.iter().map(count_single_chunk_characters).sum()
}

fn count_single_chunk_characters(chunk: &SlackAnyChunk) -> usize {
    match chunk {
        SlackAnyChunk::MarkdownText(chunk) => chunk.text.chars().count(),
        SlackAnyChunk::PlanUpdate(chunk) => chunk.title.chars().count(),
        SlackAnyChunk::TaskUpdate(chunk) => {
            let details_character_count = chunk
                .details
                .as_ref()
                .map_or(0, |details| details.chars().count());
            let output_character_count = chunk
                .output
                .as_ref()
                .map_or(0, |output| output.chars().count());
            let sources_character_count = chunk.sources.as_ref().map_or(0, |sources| {
                sources
                    .iter()
                    .map(|source| {
                        source.url.chars().count()
                            + source.text.chars().count()
                            + count_chunk_source_type_characters(&source.source_type)
                    })
                    .sum::<usize>()
            });

            chunk.id.chars().count()
                + chunk.title.chars().count()
                + count_task_status_characters(&chunk.status)
                + details_character_count
                + output_character_count
                + sources_character_count
        }
    }
}

fn count_task_status_characters(status: &SlackTaskUpdateStatus) -> usize {
    match status {
        SlackTaskUpdateStatus::Pending => "pending".chars().count(),
        SlackTaskUpdateStatus::InProgress => "in_progress".chars().count(),
        SlackTaskUpdateStatus::Complete => "complete".chars().count(),
        SlackTaskUpdateStatus::Error => "error".chars().count(),
    }
}

fn count_chunk_source_type_characters(source_type: &SlackChunkSourceType) -> usize {
    match source_type {
        SlackChunkSourceType::Url => "url".chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use crate::investigation::InvestigationLogMeta;
    use async_trait::async_trait;
    use reili_shared::errors::PortError;
    use reili_shared::ports::outbound::slack_progress_stream::SlackTaskUpdateStatus;
    use reili_shared::ports::outbound::{
        AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort,
        StartSlackProgressStreamInput, StartSlackProgressStreamOutput,
        StopSlackProgressStreamInput,
    };

    use super::{
        CreateInvestigationProgressStreamSessionFactoryInput,
        CreateInvestigationProgressStreamSessionInput,
        InvestigationProgressMessageOutputCreatedInput, InvestigationProgressReasoningInput,
        InvestigationProgressStreamSessionFactory, InvestigationProgressTaskUpdateInput,
        create_investigation_progress_stream_session_factory, is_permanent_stream_append_error,
    };
    use crate::investigation::logger::InvestigationLogger;

    struct MockSlackProgressStreamPort {
        start_calls: Mutex<Vec<StartSlackProgressStreamInput>>,
        append_calls: Mutex<Vec<AppendSlackProgressStreamInput>>,
        stop_calls: Mutex<Vec<StopSlackProgressStreamInput>>,
        start_responses: Mutex<VecDeque<Result<StartSlackProgressStreamOutput, PortError>>>,
        append_responses: Mutex<VecDeque<Result<(), PortError>>>,
        stop_responses: Mutex<VecDeque<Result<(), PortError>>>,
    }

    impl MockSlackProgressStreamPort {
        fn new() -> Self {
            Self {
                start_calls: Mutex::new(Vec::new()),
                append_calls: Mutex::new(Vec::new()),
                stop_calls: Mutex::new(Vec::new()),
                start_responses: Mutex::new(VecDeque::new()),
                append_responses: Mutex::new(VecDeque::new()),
                stop_responses: Mutex::new(VecDeque::new()),
            }
        }

        fn push_start_response(&self, response: Result<StartSlackProgressStreamOutput, PortError>) {
            self.start_responses
                .lock()
                .expect("lock start responses")
                .push_back(response);
        }

        fn push_append_response(&self, response: Result<(), PortError>) {
            self.append_responses
                .lock()
                .expect("lock append responses")
                .push_back(response);
        }

        fn push_stop_response(&self, response: Result<(), PortError>) {
            self.stop_responses
                .lock()
                .expect("lock stop responses")
                .push_back(response);
        }

        fn start_calls(&self) -> Vec<StartSlackProgressStreamInput> {
            self.start_calls.lock().expect("lock start calls").clone()
        }

        fn append_calls(&self) -> Vec<AppendSlackProgressStreamInput> {
            self.append_calls.lock().expect("lock append calls").clone()
        }

        fn stop_calls(&self) -> Vec<StopSlackProgressStreamInput> {
            self.stop_calls.lock().expect("lock stop calls").clone()
        }
    }

    #[async_trait]
    impl SlackProgressStreamPort for MockSlackProgressStreamPort {
        async fn start(
            &self,
            input: StartSlackProgressStreamInput,
        ) -> Result<StartSlackProgressStreamOutput, PortError> {
            self.start_calls
                .lock()
                .expect("lock start calls")
                .push(input);
            self.start_responses
                .lock()
                .expect("lock start responses")
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(StartSlackProgressStreamOutput {
                        stream_ts: "stream-default".to_string(),
                    })
                })
        }

        async fn append(&self, input: AppendSlackProgressStreamInput) -> Result<(), PortError> {
            self.append_calls
                .lock()
                .expect("lock append calls")
                .push(input);
            self.append_responses
                .lock()
                .expect("lock append responses")
                .pop_front()
                .unwrap_or(Ok(()))
        }

        async fn stop(&self, input: StopSlackProgressStreamInput) -> Result<(), PortError> {
            self.stop_calls.lock().expect("lock stop calls").push(input);
            self.stop_responses
                .lock()
                .expect("lock stop responses")
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
        warn_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
        error_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
    }

    impl MockLogger {
        fn info_logs(&self) -> Vec<(String, InvestigationLogMeta)> {
            self.info_logs.lock().expect("lock info logs").clone()
        }

        fn warn_logs(&self) -> Vec<(String, InvestigationLogMeta)> {
            self.warn_logs.lock().expect("lock warn logs").clone()
        }
    }

    impl InvestigationLogger for MockLogger {
        fn info(&self, message: &str, meta: InvestigationLogMeta) {
            self.info_logs
                .lock()
                .expect("lock info logs")
                .push((message.to_string(), meta));
        }

        fn warn(&self, message: &str, meta: InvestigationLogMeta) {
            self.warn_logs
                .lock()
                .expect("lock warn logs")
                .push((message.to_string(), meta));
        }

        fn error(&self, message: &str, meta: InvestigationLogMeta) {
            self.error_logs
                .lock()
                .expect("lock error logs")
                .push((message.to_string(), meta));
        }
    }

    const CHANNEL: &str = "C123";
    const THREAD_TS: &str = "123.456";
    const RECIPIENT_USER_ID: &str = "U123";

    #[tokio::test]
    async fn restarts_stream_when_append_fails_with_message_not_in_streaming_state() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-restarted".to_string(),
        }));
        stream_port.push_append_response(Err(PortError::new(
            "Error: An API error occurred: message_not_in_streaming_state",
        )));
        stream_port.push_stop_response(Ok(()));

        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );

        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                recipient_user_id: RECIPIENT_USER_ID.to_string(),
                recipient_team_id: None,
            });

        session.start().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: "coordinator".to_string(),
                title: "Collect evidence".to_string(),
                summary: "Inspect logs".to_string(),
            })
            .await;
        session.stop_as_succeeded().await;

        let start_calls = stream_port.start_calls();
        let append_calls = stream_port.append_calls();
        let stop_calls = stream_port.stop_calls();
        assert_eq!(start_calls.len(), 2);
        assert_eq!(append_calls.len(), 1);
        assert_eq!(stop_calls.len(), 1);
        assert_eq!(stop_calls[0].stream_ts, "stream-restarted");

        let restarted_logged = logger
            .info_logs()
            .iter()
            .any(|(message, _)| message == "slack_stream_restarted");
        assert!(restarted_logged);

        let restarted_chunks = start_calls[1].chunks.clone().unwrap_or_default();
        assert_eq!(restarted_chunks.len(), 1);
        match &restarted_chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::InProgress);
                assert_eq!(chunk.details.as_deref(), Some("Inspect logs\n"));
            }
            _ => panic!("expected task update chunk"),
        }
    }

    #[tokio::test]
    async fn disables_stream_when_stream_restart_fails() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        stream_port.push_start_response(Err(PortError::new("failed to restart stream")));
        stream_port.push_append_response(Err(PortError::new(
            "Error: An API error occurred: message_not_in_streaming_state",
        )));

        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );

        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                recipient_user_id: RECIPIENT_USER_ID.to_string(),
                recipient_team_id: None,
            });

        session.start().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: "coordinator".to_string(),
                title: "Collect evidence".to_string(),
                summary: String::new(),
            })
            .await;
        session.stop_as_succeeded().await;

        assert_eq!(stream_port.start_calls().len(), 2);
        assert!(stream_port.stop_calls().is_empty());
        assert_eq!(stream_port.append_calls().len(), 1);

        let disabled_logged = logger
            .warn_logs()
            .iter()
            .any(|(message, _)| message == "slack_progress_stream_disabled");
        assert!(disabled_logged);
    }

    #[tokio::test]
    async fn rotates_stream_before_append_when_character_limit_would_be_exceeded() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-rotated".to_string(),
        }));
        stream_port.push_append_response(Ok(()));
        stream_port.push_stop_response(Ok(()));
        stream_port.push_stop_response(Ok(()));

        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );

        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                recipient_user_id: RECIPIENT_USER_ID.to_string(),
                recipient_team_id: None,
            });

        session.start().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: "coordinator".to_string(),
                title: "Collect evidence".to_string(),
                summary: "a".repeat(2650),
            })
            .await;
        session
            .post_tool_started(InvestigationProgressTaskUpdateInput {
                owner_id: "coordinator".to_string(),
                task_id: "task-1".to_string(),
                title: "b".repeat(200),
            })
            .await;
        session.stop_as_succeeded().await;

        let start_calls = stream_port.start_calls();
        let append_calls = stream_port.append_calls();
        let stop_calls = stream_port.stop_calls();

        assert_eq!(start_calls.len(), 2);
        assert_eq!(append_calls.len(), 1);
        assert_eq!(stop_calls.len(), 2);
        assert_eq!(stop_calls[0].stream_ts, "stream-initial");
        assert_eq!(stop_calls[1].stream_ts, "stream-rotated");

        let rotated_chunks = start_calls[1].chunks.clone().unwrap_or_default();
        let expected_detail = format!("{}\n", "b".repeat(200));
        assert_eq!(rotated_chunks.len(), 1);
        match &rotated_chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::InProgress);
                assert_eq!(chunk.details.as_deref(), Some(expected_detail.as_str()));
            }
            _ => panic!("expected task update chunk"),
        }

        let rotation_logged = logger
            .info_logs()
            .iter()
            .any(|(message, _)| message == "slack_stream_rotated");
        assert!(rotation_logged);
    }

    #[tokio::test]
    async fn rotates_stream_when_append_fails_with_msg_too_long() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-rotated".to_string(),
        }));
        stream_port.push_append_response(Err(PortError::new(
            "Error: An API error occurred: msg_too_long",
        )));
        stream_port.push_stop_response(Ok(()));
        stream_port.push_stop_response(Ok(()));

        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );

        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                recipient_user_id: RECIPIENT_USER_ID.to_string(),
                recipient_team_id: None,
            });

        session.start().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: "coordinator".to_string(),
                title: "Collect evidence".to_string(),
                summary: "Inspect logs".to_string(),
            })
            .await;
        session.stop_as_succeeded().await;

        let start_calls = stream_port.start_calls();
        let append_calls = stream_port.append_calls();
        let stop_calls = stream_port.stop_calls();

        assert_eq!(start_calls.len(), 2);
        assert_eq!(append_calls.len(), 1);
        assert_eq!(stop_calls.len(), 2);
        assert_eq!(stop_calls[0].stream_ts, "stream-initial");
        assert_eq!(stop_calls[1].stream_ts, "stream-rotated");

        let rotated_chunks = start_calls[1].chunks.clone().unwrap_or_default();
        assert_eq!(rotated_chunks.len(), 1);
        match &rotated_chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::InProgress);
                assert_eq!(chunk.details.as_deref(), Some("Inspect logs\n"));
            }
            _ => panic!("expected task update chunk"),
        }
    }

    #[tokio::test]
    async fn completes_scope_with_reasoning_and_message_output_created_events_only() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-1".to_string(),
        }));
        stream_port.push_append_response(Ok(()));
        stream_port.push_append_response(Ok(()));
        stream_port.push_stop_response(Ok(()));

        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );
        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                recipient_user_id: RECIPIENT_USER_ID.to_string(),
                recipient_team_id: None,
            });

        session.start().await;
        session
            .post_reasoning(InvestigationProgressReasoningInput {
                owner_id: "coordinator".to_string(),
                title: "Collect evidence".to_string(),
                summary: "Inspect logs".to_string(),
            })
            .await;
        session
            .post_message_output_created(InvestigationProgressMessageOutputCreatedInput {
                owner_id: "coordinator".to_string(),
            })
            .await;
        session.stop_as_succeeded().await;

        let append_calls = stream_port.append_calls();
        assert_eq!(append_calls.len(), 2);

        let initial_chunks = append_calls[0].chunks.clone().unwrap_or_default();
        assert_eq!(initial_chunks.len(), 1);
        match &initial_chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::InProgress);
                assert_eq!(chunk.details.as_deref(), Some("Inspect logs\n"));
                assert_eq!(chunk.output, None);
            }
            _ => panic!("expected task update chunk"),
        }

        let completed_chunks = append_calls[1].chunks.clone().unwrap_or_default();
        assert_eq!(completed_chunks.len(), 1);
        match &completed_chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::Complete);
                assert_eq!(chunk.details, None);
                assert_eq!(chunk.output.as_deref(), Some("done"));
            }
            _ => panic!("expected task update chunk"),
        }
    }

    #[test]
    fn treats_invalid_arguments_as_permanent_append_error() {
        assert!(is_permanent_stream_append_error(
            "Slack API returned error: method=chat.appendStream error=invalid_arguments"
        ));
    }
}
