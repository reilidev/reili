use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use sre_shared::ports::outbound::slack_progress_stream::{
    SlackMarkdownTextChunk, SlackTaskUpdateChunk, SlackTaskUpdateStatus,
};
use sre_shared::ports::outbound::{
    AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort, SlackThreadReplyInput,
    SlackThreadReplyPort, StartSlackProgressStreamInput, StopSlackProgressStreamInput,
};

use crate::investigation::logger::InvestigationLogger;

const STREAM_START_TEXT: &str = ":hourglass_flowing_sand:";

pub struct CreateInvestigationProgressStreamSessionFactoryInput {
    pub slack_stream_reply_port: Arc<dyn SlackProgressStreamPort>,
    pub reply_port: Arc<dyn SlackThreadReplyPort>,
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
                reply_port: Arc::clone(&self.input.reply_port),
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
    reply_port: Arc<dyn SlackThreadReplyPort>,
    logger: Arc<dyn InvestigationLogger>,
    channel: String,
    thread_ts: String,
    recipient_user_id: String,
    recipient_team_id: Option<String>,
}

struct SlackInvestigationProgressStreamSession {
    input: CreateSlackInvestigationProgressStreamSessionInput,
    stream_ts: Option<String>,
    stream_stopped: bool,
    fallback_mode: bool,
    append_count: u64,
    last_error_message: Option<String>,
    active_reasoning_scope_id_by_owner_id: HashMap<String, String>,
    reasoning_scope_by_id: HashMap<String, ReasoningScope>,
    reasoning_scope_id_by_task_key: HashMap<String, String>,
    completed_reasoning_scope_ids_by_owner_id: HashMap<String, HashSet<String>>,
    latest_completed_reasoning_scope_id_by_owner_id: HashMap<String, String>,
    next_scope_number: u64,
}

impl SlackInvestigationProgressStreamSession {
    fn new(input: CreateSlackInvestigationProgressStreamSessionInput) -> Self {
        Self {
            input,
            stream_ts: None,
            stream_stopped: false,
            fallback_mode: false,
            append_count: 0,
            last_error_message: None,
            active_reasoning_scope_id_by_owner_id: HashMap::new(),
            reasoning_scope_by_id: HashMap::new(),
            reasoning_scope_id_by_task_key: HashMap::new(),
            completed_reasoning_scope_ids_by_owner_id: HashMap::new(),
            latest_completed_reasoning_scope_id_by_owner_id: HashMap::new(),
            next_scope_number: 0,
        }
    }

    async fn resolve_scope_for_tool_started(
        &mut self,
        input: &InvestigationProgressTaskUpdateInput,
    ) -> String {
        let task_ownership_key = build_task_ownership_key(&input.owner_id, &input.task_id);
        if let Some(existing_scope_id) =
            self.reasoning_scope_id_by_task_key.get(&task_ownership_key)
            && self.reasoning_scope_by_id.contains_key(existing_scope_id)
        {
            return existing_scope_id.clone();
        }

        if let Some(active_scope_id) = self
            .active_reasoning_scope_id_by_owner_id
            .get(&input.owner_id)
            .cloned()
        {
            if self.reasoning_scope_by_id.contains_key(&active_scope_id) {
                self.reasoning_scope_id_by_task_key
                    .insert(task_ownership_key, active_scope_id.clone());
                return active_scope_id;
            }
            self.active_reasoning_scope_id_by_owner_id
                .remove(&input.owner_id);
        }

        self.reopen_scope_for_tool_started(input).await
    }

    async fn resolve_scope_for_tool_completed(
        &mut self,
        input: &InvestigationProgressTaskUpdateInput,
    ) -> Option<String> {
        let task_ownership_key = build_task_ownership_key(&input.owner_id, &input.task_id);
        if let Some(existing_scope_id) =
            self.reasoning_scope_id_by_task_key.get(&task_ownership_key)
            && self.reasoning_scope_by_id.contains_key(existing_scope_id)
        {
            return Some(existing_scope_id.clone());
        }

        self.input.logger.warn(
            "reasoning_scope_not_found_for_tool_completed",
            BTreeMap::from([
                ("channel".to_string(), self.input.channel.clone()),
                ("threadTs".to_string(), self.input.thread_ts.clone()),
                ("ownerId".to_string(), input.owner_id.clone()),
                ("taskId".to_string(), input.task_id.clone()),
                ("toolName".to_string(), input.title.clone()),
            ]),
        );
        None
    }

    fn create_reasoning_scope(&mut self, owner_id: &str, title: &str) -> String {
        let scope_id = self.create_reasoning_scope_id();
        let scope = ReasoningScope {
            scope_id: scope_id.clone(),
            owner_id: owner_id.to_string(),
            title: title.to_string(),
            tool_status_by_task_id: HashMap::new(),
        };
        self.reasoning_scope_by_id.insert(scope_id.clone(), scope);
        scope_id
    }

    fn create_reasoning_scope_id(&mut self) -> String {
        self.next_scope_number = self.next_scope_number.saturating_add(1);
        let mut scope_id = format!("reasoning-{}", self.next_scope_number);
        while self.reasoning_scope_by_id.contains_key(&scope_id) {
            self.next_scope_number = self.next_scope_number.saturating_add(1);
            scope_id = format!("reasoning-{}", self.next_scope_number);
        }

        scope_id
    }

    fn upsert_scope_tool_status(
        &mut self,
        scope_id: &str,
        owner_id: &str,
        task_id: &str,
        status: ReasoningScopeToolStatus,
    ) {
        if let Some(scope) = self.reasoning_scope_by_id.get_mut(scope_id) {
            scope
                .tool_status_by_task_id
                .insert(task_id.to_string(), status.clone());
        }

        self.reasoning_scope_id_by_task_key.insert(
            build_task_ownership_key(owner_id, task_id),
            scope_id.to_string(),
        );
    }

    async fn complete_active_reasoning_scope_if_idle(&mut self, owner_id: &str) {
        let Some(active_scope_id) = self
            .active_reasoning_scope_id_by_owner_id
            .get(owner_id)
            .cloned()
        else {
            return;
        };

        let Some(active_scope) = self.reasoning_scope_by_id.get(&active_scope_id).cloned() else {
            self.active_reasoning_scope_id_by_owner_id.remove(owner_id);
            return;
        };

        if scope_has_in_progress_tool(&active_scope) {
            return;
        }

        self.complete_scope_if_needed(&active_scope_id).await;
    }

    async fn complete_scope_if_needed(&mut self, scope_id: &str) {
        let Some(scope) = self.reasoning_scope_by_id.get(scope_id).cloned() else {
            return;
        };

        {
            let completed_scope_ids =
                self.resolve_completed_reasoning_scope_ids_by_owner_id(&scope.owner_id);
            if completed_scope_ids.contains(scope_id) {
                return;
            }
            completed_scope_ids.insert(scope_id.to_string());
        }

        self.latest_completed_reasoning_scope_id_by_owner_id
            .insert(scope.owner_id.clone(), scope_id.to_string());

        self.append_reasoning_scope_update(&scope, ReasoningScopeStatus::Complete, None)
            .await;
    }

    async fn reopen_scope_for_tool_started(
        &mut self,
        input: &InvestigationProgressTaskUpdateInput,
    ) -> String {
        let last_completed_scope = self.resolve_latest_completed_scope_by_owner_id(&input.owner_id);
        let reopened_scope_title = last_completed_scope.as_ref().map_or_else(
            || "Tool executions".to_string(),
            |scope| scope.title.clone(),
        );
        let reopened_scope_id = self.create_reasoning_scope(&input.owner_id, &reopened_scope_title);
        self.active_reasoning_scope_id_by_owner_id
            .insert(input.owner_id.clone(), reopened_scope_id.clone());

        let mut meta = BTreeMap::from([
            ("channel".to_string(), self.input.channel.clone()),
            ("threadTs".to_string(), self.input.thread_ts.clone()),
            ("ownerId".to_string(), input.owner_id.clone()),
            ("taskId".to_string(), input.task_id.clone()),
            ("toolName".to_string(), input.title.clone()),
            ("reopenedScopeId".to_string(), reopened_scope_id.clone()),
        ]);
        if let Some(scope) = last_completed_scope {
            meta.insert("reopenedFromScopeId".to_string(), scope.scope_id);
        }
        self.input
            .logger
            .info("reasoning_scope_reopened_for_tool_started", meta);

        reopened_scope_id
    }

    fn resolve_latest_completed_scope_by_owner_id(
        &mut self,
        owner_id: &str,
    ) -> Option<ReasoningScope> {
        let latest_completed_scope_id = self
            .latest_completed_reasoning_scope_id_by_owner_id
            .get(owner_id)
            .cloned()?;

        let scope = self
            .reasoning_scope_by_id
            .get(&latest_completed_scope_id)
            .cloned();
        if scope.is_none() {
            self.latest_completed_reasoning_scope_id_by_owner_id
                .remove(owner_id);
        }

        scope
    }

    fn resolve_completed_reasoning_scope_ids_by_owner_id(
        &mut self,
        owner_id: &str,
    ) -> &mut HashSet<String> {
        self.completed_reasoning_scope_ids_by_owner_id
            .entry(owner_id.to_string())
            .or_default()
    }

    async fn append_reasoning_scope_update(
        &mut self,
        scope: &ReasoningScope,
        status: ReasoningScopeStatus,
        detail_line: Option<String>,
    ) {
        let chunk = SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: scope.scope_id.clone(),
            title: scope.title.clone(),
            status: to_slack_task_status(&status),
            details: detail_line.clone(),
            output: if status == ReasoningScopeStatus::Complete {
                Some("done".to_string())
            } else {
                None
            },
            sources: None,
        });

        self.append(
            vec![chunk],
            build_reasoning_scope_fallback_text(scope, &status, detail_line.as_deref()),
        )
        .await;
    }

    async fn stop(&mut self) {
        if self.stream_stopped {
            return;
        }

        if self.fallback_mode || self.stream_ts.is_none() {
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
                BTreeMap::from([
                    ("channel".to_string(), self.input.channel.clone()),
                    ("threadTs".to_string(), self.input.thread_ts.clone()),
                    ("streamTs".to_string(), stream_ts),
                    ("error".to_string(), error.message.clone()),
                    ("slack_stream_last_error".to_string(), error.message),
                ]),
            );
        }

        self.stream_stopped = true;
        self.log_stop();
    }

    async fn append(&mut self, chunks: Vec<SlackAnyChunk>, fallback_text: String) {
        if self.stream_stopped {
            return;
        }

        if self.fallback_mode || self.stream_ts.is_none() {
            self.post_fallback_message(&fallback_text).await;
            return;
        }

        let stream_ts = self.stream_ts.clone().unwrap_or_default();
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
            return;
        }

        let error_message = append_result
            .expect_err("append_result should be error")
            .message;
        self.last_error_message = Some(error_message.clone());
        self.input.logger.warn(
            "Failed to append Slack progress stream",
            BTreeMap::from([
                ("channel".to_string(), self.input.channel.clone()),
                ("threadTs".to_string(), self.input.thread_ts.clone()),
                ("streamTs".to_string(), stream_ts.clone()),
                ("error".to_string(), error_message.clone()),
                ("slack_stream_last_error".to_string(), error_message.clone()),
            ]),
        );

        if is_message_not_in_streaming_state_error(&error_message) {
            self.restart_stream_with_chunks(chunks, fallback_text, Some(stream_ts), error_message)
                .await;
            return;
        }

        if is_permanent_stream_append_error(&error_message) {
            self.enable_fallback_mode(error_message, "append_failed_permanent")
                .await;
            self.post_fallback_message(&fallback_text).await;
        }
    }

    async fn restart_stream_with_chunks(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
        fallback_text: String,
        failed_stream_ts: Option<String>,
        error_message: String,
    ) {
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
            self.enable_fallback_mode(restart_error_message, "append_failed_stream_restart_failed")
                .await;
            self.post_fallback_message(&fallback_text).await;
            return;
        };

        self.stream_ts = Some(stream.stream_ts.clone());
        self.append_count = self.append_count.saturating_add(1);

        let mut meta = BTreeMap::from([
            ("channel".to_string(), self.input.channel.clone()),
            ("threadTs".to_string(), self.input.thread_ts.clone()),
            ("streamTs".to_string(), stream.stream_ts),
            ("error".to_string(), error_message.clone()),
            ("slack_stream_last_error".to_string(), error_message),
        ]);
        if let Some(previous_stream_ts) = failed_stream_ts {
            meta.insert("previousStreamTs".to_string(), previous_stream_ts);
        }
        self.input.logger.info("slack_stream_restarted", meta);
    }

    async fn enable_fallback_mode(&mut self, error_message: String, reason: &str) {
        self.fallback_mode = true;
        self.last_error_message = Some(error_message.clone());

        let mut meta = BTreeMap::from([
            ("channel".to_string(), self.input.channel.clone()),
            ("threadTs".to_string(), self.input.thread_ts.clone()),
            ("reason".to_string(), reason.to_string()),
            ("error".to_string(), error_message.clone()),
            (
                "slack_stream_fallback_mode".to_string(),
                self.fallback_mode.to_string(),
            ),
            ("slack_stream_last_error".to_string(), error_message),
        ]);
        if let Some(stream_ts) = &self.stream_ts {
            meta.insert("streamTs".to_string(), stream_ts.clone());
        }
        self.input.logger.warn("slack_stream_fallback_mode", meta);
    }

    async fn post_fallback_message(&self, text: &str) {
        let post_result = self
            .input
            .reply_port
            .post_thread_reply(SlackThreadReplyInput {
                channel: self.input.channel.clone(),
                thread_ts: self.input.thread_ts.clone(),
                text: text.to_string(),
            })
            .await;

        if let Err(error) = post_result {
            self.input.logger.warn(
                "Failed to post fallback progress message",
                BTreeMap::from([
                    ("channel".to_string(), self.input.channel.clone()),
                    ("threadTs".to_string(), self.input.thread_ts.clone()),
                    ("error".to_string(), error.message),
                ]),
            );
        }
    }

    fn log_stop(&self) {
        let mut meta = BTreeMap::from([
            ("channel".to_string(), self.input.channel.clone()),
            ("threadTs".to_string(), self.input.thread_ts.clone()),
            (
                "slack_stream_append_count".to_string(),
                self.append_count.to_string(),
            ),
            (
                "slack_stream_fallback_mode".to_string(),
                self.fallback_mode.to_string(),
            ),
        ]);
        if let Some(stream_ts) = &self.stream_ts {
            meta.insert("streamTs".to_string(), stream_ts.clone());
        }
        if let Some(last_error_message) = &self.last_error_message {
            meta.insert(
                "slack_stream_last_error".to_string(),
                last_error_message.clone(),
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
                let mut meta = BTreeMap::from([
                    ("channel".to_string(), self.input.channel.clone()),
                    ("threadTs".to_string(), self.input.thread_ts.clone()),
                    ("streamTs".to_string(), stream.stream_ts),
                    (
                        "slack_stream_fallback_mode".to_string(),
                        self.fallback_mode.to_string(),
                    ),
                ]);
                if let Some(last_error_message) = &self.last_error_message {
                    meta.insert(
                        "slack_stream_last_error".to_string(),
                        last_error_message.clone(),
                    );
                }
                self.input.logger.info("slack_stream_started", meta);
            }
            Err(error) => {
                self.enable_fallback_mode(error.message, "start_failed")
                    .await;
                self.post_fallback_message(STREAM_START_TEXT).await;
            }
        }
    }

    async fn post_reasoning(&mut self, input: InvestigationProgressReasoningInput) {
        if input.title.trim().is_empty() {
            return;
        }

        self.complete_active_reasoning_scope_if_idle(&input.owner_id)
            .await;

        let scope_id = self.create_reasoning_scope(&input.owner_id, &input.title);
        self.active_reasoning_scope_id_by_owner_id
            .insert(input.owner_id, scope_id.clone());

        if let Some(scope) = self.reasoning_scope_by_id.get(&scope_id).cloned() {
            self.append_reasoning_scope_update(
                &scope,
                ReasoningScopeStatus::InProgress,
                normalize_reasoning_summary(&input.summary),
            )
            .await;
        }
    }

    async fn post_tool_started(&mut self, input: InvestigationProgressTaskUpdateInput) {
        let scope_id = self.resolve_scope_for_tool_started(&input).await;
        self.upsert_scope_tool_status(
            &scope_id,
            &input.owner_id,
            &input.task_id,
            ReasoningScopeToolStatus::InProgress,
        );

        self.resolve_completed_reasoning_scope_ids_by_owner_id(&input.owner_id)
            .remove(&scope_id);

        if let Some(scope) = self.reasoning_scope_by_id.get(&scope_id).cloned() {
            self.append_reasoning_scope_update(
                &scope,
                ReasoningScopeStatus::InProgress,
                Some(build_tool_detail_line(&input.title)),
            )
            .await;
        }
    }

    async fn post_tool_completed(&mut self, input: InvestigationProgressTaskUpdateInput) {
        let Some(scope_id) = self.resolve_scope_for_tool_completed(&input).await else {
            return;
        };

        self.upsert_scope_tool_status(
            &scope_id,
            &input.owner_id,
            &input.task_id,
            ReasoningScopeToolStatus::Complete,
        );

        let Some(scope) = self.reasoning_scope_by_id.get(&scope_id).cloned() else {
            return;
        };

        if resolve_reasoning_scope_status(&scope) == ReasoningScopeStatus::Complete {
            return;
        }

        self.append_reasoning_scope_update(&scope, ReasoningScopeStatus::InProgress, None)
            .await;
    }

    async fn post_message_output_created(
        &mut self,
        input: InvestigationProgressMessageOutputCreatedInput,
    ) {
        let scope_ids: Vec<String> = self
            .reasoning_scope_by_id
            .values()
            .filter(|scope| scope.owner_id == input.owner_id)
            .map(|scope| scope.scope_id.clone())
            .collect();

        for scope_id in scope_ids {
            self.complete_scope_if_needed(&scope_id).await;
        }

        self.active_reasoning_scope_id_by_owner_id
            .remove(&input.owner_id);
    }

    async fn stop_as_succeeded(&mut self) {
        self.stop().await;
    }

    async fn stop_as_failed(&mut self) {
        self.stop().await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReasoningScopeStatus {
    InProgress,
    Complete,
}

type ReasoningScopeToolStatus = ReasoningScopeStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReasoningScope {
    scope_id: String,
    owner_id: String,
    title: String,
    tool_status_by_task_id: HashMap<String, ReasoningScopeToolStatus>,
}

fn to_slack_task_status(status: &ReasoningScopeStatus) -> SlackTaskUpdateStatus {
    match status {
        ReasoningScopeStatus::InProgress => SlackTaskUpdateStatus::InProgress,
        ReasoningScopeStatus::Complete => SlackTaskUpdateStatus::Complete,
    }
}

fn build_task_ownership_key(owner_id: &str, task_id: &str) -> String {
    format!("{owner_id}:{task_id}")
}

fn scope_has_in_progress_tool(scope: &ReasoningScope) -> bool {
    scope
        .tool_status_by_task_id
        .values()
        .any(|status| *status == ReasoningScopeToolStatus::InProgress)
}

fn resolve_reasoning_scope_status(scope: &ReasoningScope) -> ReasoningScopeStatus {
    if scope.tool_status_by_task_id.is_empty() {
        return ReasoningScopeStatus::InProgress;
    }

    if scope
        .tool_status_by_task_id
        .values()
        .any(|status| *status == ReasoningScopeToolStatus::InProgress)
    {
        return ReasoningScopeStatus::InProgress;
    }

    ReasoningScopeStatus::Complete
}

fn build_reasoning_scope_fallback_text(
    scope: &ReasoningScope,
    status: &ReasoningScopeStatus,
    detail_line: Option<&str>,
) -> String {
    let details_text = detail_line.map_or_else(String::new, |detail| format!("\n{detail}"));
    if *status == ReasoningScopeStatus::Complete {
        return format!(
            ":white_check_mark: {} が完了しました{details_text}",
            scope.title
        );
    }

    format!(":hammer_and_wrench: {}{details_text}", scope.title)
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

fn is_message_not_in_streaming_state_error(error_message: &str) -> bool {
    error_message
        .to_lowercase()
        .contains("message_not_in_streaming_state")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use sre_shared::errors::PortError;
    use sre_shared::ports::outbound::slack_progress_stream::SlackTaskUpdateStatus;
    use sre_shared::ports::outbound::{
        AppendSlackProgressStreamInput, SlackAnyChunk, SlackProgressStreamPort,
        SlackThreadReplyInput, SlackThreadReplyPort, StartSlackProgressStreamInput,
        StartSlackProgressStreamOutput, StopSlackProgressStreamInput,
    };

    use super::{
        CreateInvestigationProgressStreamSessionFactoryInput,
        CreateInvestigationProgressStreamSessionInput,
        InvestigationProgressMessageOutputCreatedInput, InvestigationProgressReasoningInput,
        InvestigationProgressStreamSessionFactory,
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
    struct MockSlackThreadReplyPort {
        calls: Mutex<Vec<SlackThreadReplyInput>>,
    }

    impl MockSlackThreadReplyPort {
        fn calls(&self) -> Vec<SlackThreadReplyInput> {
            self.calls.lock().expect("lock reply calls").clone()
        }
    }

    #[async_trait]
    impl SlackThreadReplyPort for MockSlackThreadReplyPort {
        async fn post_thread_reply(&self, input: SlackThreadReplyInput) -> Result<(), PortError> {
            self.calls.lock().expect("lock reply calls").push(input);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
        warn_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
        error_logs: Mutex<Vec<(String, BTreeMap<String, String>)>>,
    }

    impl MockLogger {
        fn info_logs(&self) -> Vec<(String, BTreeMap<String, String>)> {
            self.info_logs.lock().expect("lock info logs").clone()
        }

        fn warn_logs(&self) -> Vec<(String, BTreeMap<String, String>)> {
            self.warn_logs.lock().expect("lock warn logs").clone()
        }
    }

    impl InvestigationLogger for MockLogger {
        fn info(&self, message: &str, meta: BTreeMap<String, String>) {
            self.info_logs
                .lock()
                .expect("lock info logs")
                .push((message.to_string(), meta));
        }

        fn warn(&self, message: &str, meta: BTreeMap<String, String>) {
            self.warn_logs
                .lock()
                .expect("lock warn logs")
                .push((message.to_string(), meta));
        }

        fn error(&self, message: &str, meta: BTreeMap<String, String>) {
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

        let reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                reply_port: Arc::clone(&reply_port) as Arc<dyn SlackThreadReplyPort>,
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
        assert!(reply_port.calls().is_empty());

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
    async fn falls_back_to_thread_reply_when_stream_restart_fails() {
        let stream_port = Arc::new(MockSlackProgressStreamPort::new());
        stream_port.push_start_response(Ok(StartSlackProgressStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        stream_port.push_start_response(Err(PortError::new("failed to restart stream")));
        stream_port.push_append_response(Err(PortError::new(
            "Error: An API error occurred: message_not_in_streaming_state",
        )));

        let reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                reply_port: Arc::clone(&reply_port) as Arc<dyn SlackThreadReplyPort>,
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

        let reply_calls = reply_port.calls();
        assert_eq!(reply_calls.len(), 1);
        assert_eq!(
            reply_calls[0],
            SlackThreadReplyInput {
                channel: CHANNEL.to_string(),
                thread_ts: THREAD_TS.to_string(),
                text: ":hammer_and_wrench: Collect evidence".to_string(),
            }
        );

        let fallback_logged = logger
            .warn_logs()
            .iter()
            .any(|(message, _)| message == "slack_stream_fallback_mode");
        assert!(fallback_logged);
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

        let reply_port = Arc::new(MockSlackThreadReplyPort::default());
        let logger = Arc::new(MockLogger::default());

        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                slack_stream_reply_port: Arc::clone(&stream_port)
                    as Arc<dyn SlackProgressStreamPort>,
                reply_port: Arc::clone(&reply_port) as Arc<dyn SlackThreadReplyPort>,
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
