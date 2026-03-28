use std::sync::Arc;
use std::time::Instant;

use reili_core::error::PortError;
use reili_core::task::StartTaskProgressSessionInput;

use super::chunk_rotation::{
    STREAM_ROTATION_CHARACTER_LIMIT, STREAM_ROTATION_MAX_AGE, count_chunk_characters,
    should_rotate_stream,
};
use super::{
    LogFieldValue, SlackAnyChunk, SlackAppendStreamInput, SlackProgressStreamApiPort,
    SlackProgressStreamLogger, SlackStartStreamInput, SlackStopStreamInput, string_log_meta,
};

pub(crate) struct SlackProgressStreamLifecycle {
    api: Arc<dyn SlackProgressStreamApiPort>,
    logger: Arc<dyn SlackProgressStreamLogger>,
    clock: Arc<dyn SlackProgressStreamClock>,
    route: StartTaskProgressSessionInput,
    stream_ts: Option<String>,
    stream_started_at: Option<Instant>,
    current_stream_character_count: usize,
    stream_stopped: bool,
    append_count: u64,
    last_error_message: Option<String>,
}

struct RecoverFromAppendFailureInput {
    chunks: Vec<SlackAnyChunk>,
    failed_stream_ts: String,
    error: PortError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlackProgressStreamAppendOutcome {
    Ignored,
    Appended,
    RotationRequired(SlackStreamRotationReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackProgressStreamRotationInput {
    pub stop_chunks: Vec<SlackAnyChunk>,
    pub start_chunks: Vec<SlackAnyChunk>,
    pub append_chunks: Option<Vec<SlackAnyChunk>>,
    pub reason: SlackStreamRotationReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlackStreamRotationReason {
    CharacterLimit,
    TimeLimit,
    MessageTooLong,
}

impl SlackStreamRotationReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::CharacterLimit => "character_limit",
            Self::TimeLimit => "time_limit",
            Self::MessageTooLong => "msg_too_long",
        }
    }
}

pub(crate) trait SlackProgressStreamClock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemSlackProgressStreamClock;

impl SlackProgressStreamClock for SystemSlackProgressStreamClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

impl SlackProgressStreamLifecycle {
    pub(crate) fn new(
        api: Arc<dyn SlackProgressStreamApiPort>,
        logger: Arc<dyn SlackProgressStreamLogger>,
        route: StartTaskProgressSessionInput,
    ) -> Self {
        Self::new_with_clock(api, logger, route, Arc::new(SystemSlackProgressStreamClock))
    }

    pub(crate) fn new_with_clock(
        api: Arc<dyn SlackProgressStreamApiPort>,
        logger: Arc<dyn SlackProgressStreamLogger>,
        route: StartTaskProgressSessionInput,
        clock: Arc<dyn SlackProgressStreamClock>,
    ) -> Self {
        Self {
            api,
            logger,
            clock,
            route,
            stream_ts: None,
            stream_started_at: None,
            current_stream_character_count: 0,
            stream_stopped: false,
            append_count: 0,
            last_error_message: None,
        }
    }

    pub(crate) async fn start(&mut self, chunks: Vec<SlackAnyChunk>) {
        if self.stream_stopped || self.stream_ts.is_some() {
            return;
        }

        let start_result = self
            .api
            .start(SlackStartStreamInput {
                channel: self.route.channel.clone(),
                thread_ts: self.route.thread_ts.clone(),
                recipient_user_id: self.route.recipient_user_id.clone(),
                recipient_team_id: self.route.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(chunks.clone()),
            })
            .await;

        match start_result {
            Ok(stream) => {
                self.record_started_stream(
                    stream.stream_ts.clone(),
                    count_chunk_characters(&chunks),
                );
                let mut meta = string_log_meta([
                    ("channel", self.route.channel.clone()),
                    ("threadTs", self.route.thread_ts.clone()),
                    ("streamTs", stream.stream_ts),
                ]);
                if let Some(last_error_message) = &self.last_error_message {
                    meta.insert(
                        "slack_stream_last_error".to_string(),
                        LogFieldValue::from(last_error_message.clone()),
                    );
                }
                self.logger.info("slack_stream_started", meta);
            }
            Err(error) => {
                self.log_start_failure(error.message);
            }
        }
    }

    pub(crate) fn rotation_reason_for_append(
        &self,
        chunks: &[SlackAnyChunk],
    ) -> Option<SlackStreamRotationReason> {
        if self.stream_stopped || self.stream_ts.is_none() {
            return None;
        }

        self.rotation_reason(chunks)
    }

    pub(crate) async fn append(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
    ) -> SlackProgressStreamAppendOutcome {
        if self.stream_stopped || self.stream_ts.is_none() {
            return SlackProgressStreamAppendOutcome::Ignored;
        }

        if let Some(reason) = self.rotation_reason(&chunks) {
            return SlackProgressStreamAppendOutcome::RotationRequired(reason);
        }

        self.append_chunks_to_current_stream(chunks).await
    }

    pub(crate) async fn rotate(&mut self, input: SlackProgressStreamRotationInput) {
        if self.stream_stopped || self.stream_ts.is_none() {
            return;
        }

        let previous_stream_ts = self.stream_ts.clone();
        let previous_character_count = self.current_stream_character_count;
        let previous_stream_age = self.current_stream_elapsed();
        let start_chunk_character_count = count_chunk_characters(&input.start_chunks);

        self.stop_current_stream_for_rotation(input.stop_chunks)
            .await;

        let start_result = self
            .api
            .start(SlackStartStreamInput {
                channel: self.route.channel.clone(),
                thread_ts: self.route.thread_ts.clone(),
                recipient_user_id: self.route.recipient_user_id.clone(),
                recipient_team_id: self.route.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(input.start_chunks),
            })
            .await;

        let Ok(stream) = start_result else {
            let error_message = start_result
                .expect_err("start_result should be error")
                .message;
            self.disable_stream(error_message, "stream_rotation_start_failed");
            return;
        };

        self.record_started_stream(stream.stream_ts.clone(), start_chunk_character_count);

        if let Some(append_chunks) = input.append_chunks {
            if matches!(
                self.append_chunks_to_current_stream(append_chunks).await,
                SlackProgressStreamAppendOutcome::RotationRequired(_)
            ) {
                self.disable_stream(
                    self.last_error_message.clone().unwrap_or_else(|| {
                        "Slack API returned msg_too_long while appending rotated stream".to_string()
                    }),
                    "stream_rotation_append_failed",
                );
                return;
            }
        } else {
            self.append_count = self.append_count.saturating_add(1);
        }

        let mut meta = string_log_meta([
            ("channel", self.route.channel.clone()),
            ("threadTs", self.route.thread_ts.clone()),
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
                "previousSlackStreamAgeSeconds",
                previous_stream_age.as_secs().to_string(),
            ),
            (
                "slack_stream_max_age_seconds",
                STREAM_ROTATION_MAX_AGE.as_secs().to_string(),
            ),
            ("reason", input.reason.as_str().to_string()),
            (
                "previousSlackStreamCharacterCount",
                previous_character_count.to_string(),
            ),
        ]);
        if let Some(previous_stream_ts) = previous_stream_ts {
            meta.insert(
                "previousStreamTs".to_string(),
                LogFieldValue::from(previous_stream_ts),
            );
        }
        self.logger.info("slack_stream_rotated", meta);
    }

    pub(crate) async fn stop(&mut self) {
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
            .api
            .stop(SlackStopStreamInput {
                channel: self.route.channel.clone(),
                stream_ts: stream_ts.clone(),
                markdown_text: None,
                chunks: None,
                blocks: None,
            })
            .await;
        if let Err(error) = stop_result {
            self.last_error_message = Some(error.message.clone());
            self.logger.warn(
                "Failed to stop Slack progress stream",
                string_log_meta([
                    ("channel", self.route.channel.clone()),
                    ("threadTs", self.route.thread_ts.clone()),
                    ("streamTs", stream_ts),
                    ("error", error.message.clone()),
                    ("slack_stream_last_error", error.message),
                ]),
            );
        }

        self.stream_stopped = true;
        self.log_stop();
    }

    async fn append_chunks_to_current_stream(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
    ) -> SlackProgressStreamAppendOutcome {
        let stream_ts = self.stream_ts.clone().unwrap_or_default();
        let chunk_character_count = count_chunk_characters(&chunks);
        let append_result = self
            .api
            .append(SlackAppendStreamInput {
                channel: self.route.channel.clone(),
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
            return SlackProgressStreamAppendOutcome::Appended;
        }

        self.recover_from_append_failure(RecoverFromAppendFailureInput {
            chunks,
            failed_stream_ts: stream_ts,
            error: append_result.expect_err("append_result should be error"),
        })
        .await
    }

    async fn recover_from_append_failure(
        &mut self,
        input: RecoverFromAppendFailureInput,
    ) -> SlackProgressStreamAppendOutcome {
        self.last_error_message = Some(input.error.message.clone());
        self.logger.warn(
            "Failed to append Slack progress stream",
            string_log_meta([
                ("channel", self.route.channel.clone()),
                ("threadTs", self.route.thread_ts.clone()),
                ("streamTs", input.failed_stream_ts.clone()),
                ("error", input.error.message.clone()),
                ("slack_stream_last_error", input.error.message.clone()),
            ]),
        );

        if is_message_not_in_streaming_state_error(&input.error) {
            return self
                .restart_stream_with_chunks(
                    input.chunks,
                    Some(input.failed_stream_ts),
                    input.error.message,
                )
                .await;
        }

        if is_message_too_long_error(&input.error) {
            return SlackProgressStreamAppendOutcome::RotationRequired(
                SlackStreamRotationReason::MessageTooLong,
            );
        }

        if is_permanent_stream_append_error(&input.error) {
            self.disable_stream(input.error.message, "append_failed_permanent");
        }

        SlackProgressStreamAppendOutcome::Ignored
    }

    async fn restart_stream_with_chunks(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
        failed_stream_ts: Option<String>,
        error_message: String,
    ) -> SlackProgressStreamAppendOutcome {
        let chunk_character_count = count_chunk_characters(&chunks);
        let start_result = self
            .api
            .start(SlackStartStreamInput {
                channel: self.route.channel.clone(),
                thread_ts: self.route.thread_ts.clone(),
                recipient_user_id: self.route.recipient_user_id.clone(),
                recipient_team_id: self.route.recipient_team_id.clone(),
                markdown_text: None,
                chunks: Some(chunks),
            })
            .await;

        let Ok(stream) = start_result else {
            let restart_error_message = start_result
                .expect_err("start_result should be error")
                .message;
            self.disable_stream(restart_error_message, "append_failed_stream_restart_failed");
            return SlackProgressStreamAppendOutcome::Ignored;
        };

        self.record_started_stream(stream.stream_ts.clone(), chunk_character_count);
        self.append_count = self.append_count.saturating_add(1);

        let mut meta = string_log_meta([
            ("channel", self.route.channel.clone()),
            ("threadTs", self.route.thread_ts.clone()),
            ("streamTs", stream.stream_ts),
            ("error", error_message.clone()),
            ("slack_stream_last_error", error_message),
        ]);
        if let Some(previous_stream_ts) = failed_stream_ts {
            meta.insert(
                "previousStreamTs".to_string(),
                LogFieldValue::from(previous_stream_ts),
            );
        }
        self.logger.info("slack_stream_restarted", meta);

        SlackProgressStreamAppendOutcome::Appended
    }

    async fn stop_current_stream_for_rotation(&mut self, chunks: Vec<SlackAnyChunk>) {
        let Some(stream_ts) = self.stream_ts.clone() else {
            return;
        };

        let stop_result = self
            .api
            .stop(SlackStopStreamInput {
                channel: self.route.channel.clone(),
                stream_ts: stream_ts.clone(),
                markdown_text: None,
                chunks: (!chunks.is_empty()).then_some(chunks),
                blocks: None,
            })
            .await;

        if let Err(error) = stop_result {
            self.last_error_message = Some(error.message.clone());
            self.logger.warn(
                "Failed to stop Slack progress stream before rotation",
                string_log_meta([
                    ("channel", self.route.channel.clone()),
                    ("threadTs", self.route.thread_ts.clone()),
                    ("streamTs", stream_ts),
                    ("error", error.message.clone()),
                    ("slack_stream_last_error", error.message),
                ]),
            );
        }
    }

    fn disable_stream(&mut self, error_message: String, reason: &str) {
        self.stream_ts = None;
        self.stream_started_at = None;
        self.current_stream_character_count = 0;
        self.last_error_message = Some(error_message.clone());

        self.logger.warn(
            "slack_progress_stream_disabled",
            string_log_meta([
                ("channel", self.route.channel.clone()),
                ("threadTs", self.route.thread_ts.clone()),
                ("reason", reason.to_string()),
                ("error", error_message.clone()),
                ("slack_stream_last_error", error_message),
            ]),
        );
    }

    fn log_start_failure(&mut self, error_message: String) {
        self.last_error_message = Some(error_message.clone());
        self.logger.warn(
            "Failed to start Slack progress stream",
            string_log_meta([
                ("channel", self.route.channel.clone()),
                ("threadTs", self.route.thread_ts.clone()),
                ("error", error_message.clone()),
                ("slack_stream_last_error", error_message),
            ]),
        );
    }

    fn log_stop(&self) {
        let mut meta = string_log_meta([
            ("channel", self.route.channel.clone()),
            ("threadTs", self.route.thread_ts.clone()),
            ("slack_stream_append_count", self.append_count.to_string()),
        ]);
        if let Some(stream_ts) = &self.stream_ts {
            meta.insert(
                "streamTs".to_string(),
                LogFieldValue::from(stream_ts.clone()),
            );
        }
        if let Some(last_error_message) = &self.last_error_message {
            meta.insert(
                "slack_stream_last_error".to_string(),
                LogFieldValue::from(last_error_message.clone()),
            );
        }

        self.logger.info("slack_stream_stopped", meta);
    }

    fn record_started_stream(&mut self, stream_ts: String, character_count: usize) {
        self.stream_ts = Some(stream_ts);
        self.stream_started_at = Some(self.clock.now());
        self.current_stream_character_count = character_count;
    }

    fn current_stream_elapsed(&self) -> std::time::Duration {
        self.stream_started_at
            .map(|started_at| self.clock.now().saturating_duration_since(started_at))
            .unwrap_or_default()
    }

    fn rotation_reason(&self, chunks: &[SlackAnyChunk]) -> Option<SlackStreamRotationReason> {
        let current_stream_elapsed = self.current_stream_elapsed();
        if current_stream_elapsed >= STREAM_ROTATION_MAX_AGE {
            return Some(SlackStreamRotationReason::TimeLimit);
        }

        if should_rotate_stream(
            self.current_stream_character_count,
            current_stream_elapsed,
            chunks,
        ) {
            return Some(SlackStreamRotationReason::CharacterLimit);
        }

        None
    }
}

fn is_permanent_stream_append_error(error: &PortError) -> bool {
    error.is_service_error_code("invalid_ts")
        || error.is_service_error_code("message_not_found")
        || error.is_service_error_code("channel_not_found")
        || error.is_service_error_code("invalid_arguments")
}

fn is_message_too_long_error(error: &PortError) -> bool {
    error.is_service_error_code("msg_too_long")
}

fn is_message_not_in_streaming_state_error(error: &PortError) -> bool {
    error.is_service_error_code("message_not_in_streaming_state")
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::logger::{LogEntry, LogFields, LogLevel};
    use reili_core::task::StartTaskProgressSessionInput;

    use super::{
        SlackProgressStreamAppendOutcome, SlackProgressStreamClock, SlackProgressStreamLifecycle,
        SlackProgressStreamRotationInput, SlackStreamRotationReason,
    };
    use crate::outbound::slack::progress_stream::{
        LogFieldValue, SlackAnyChunk, SlackAppendStreamInput, SlackMarkdownTextChunk,
        SlackProgressStreamApiPort, SlackProgressStreamLogger, SlackStartStreamInput,
        SlackStartStreamOutput, SlackStopStreamInput,
    };

    struct MockSlackProgressStreamApi {
        start_calls: Mutex<Vec<SlackStartStreamInput>>,
        append_calls: Mutex<Vec<SlackAppendStreamInput>>,
        stop_calls: Mutex<Vec<SlackStopStreamInput>>,
        start_responses: Mutex<VecDeque<Result<SlackStartStreamOutput, PortError>>>,
        append_responses: Mutex<VecDeque<Result<(), PortError>>>,
        stop_responses: Mutex<VecDeque<Result<(), PortError>>>,
    }

    impl MockSlackProgressStreamApi {
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

        fn push_start_response(&self, response: Result<SlackStartStreamOutput, PortError>) {
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
    }

    #[async_trait]
    impl SlackProgressStreamApiPort for MockSlackProgressStreamApi {
        async fn start(
            &self,
            input: SlackStartStreamInput,
        ) -> Result<SlackStartStreamOutput, PortError> {
            self.start_calls
                .lock()
                .expect("lock start calls")
                .push(input);
            self.start_responses
                .lock()
                .expect("lock start responses")
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(SlackStartStreamOutput {
                        stream_ts: "stream-default".to_string(),
                    })
                })
        }

        async fn append(&self, input: SlackAppendStreamInput) -> Result<(), PortError> {
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

        async fn stop(&self, input: SlackStopStreamInput) -> Result<(), PortError> {
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
        info_logs: Mutex<Vec<(String, LogFields)>>,
        warn_logs: Mutex<Vec<(String, LogFields)>>,
    }

    impl MockLogger {
        fn info_logs(&self) -> Vec<(String, LogFields)> {
            self.info_logs.lock().expect("lock info logs").clone()
        }

        fn warn_logs(&self) -> Vec<(String, LogFields)> {
            self.warn_logs.lock().expect("lock warn logs").clone()
        }
    }

    impl SlackProgressStreamLogger for MockLogger {
        fn log(&self, entry: LogEntry) {
            match entry.level {
                LogLevel::Info => self
                    .info_logs
                    .lock()
                    .expect("lock info logs")
                    .push((entry.event.to_string(), entry.fields)),
                LogLevel::Warn => self
                    .warn_logs
                    .lock()
                    .expect("lock warn logs")
                    .push((entry.event.to_string(), entry.fields)),
                LogLevel::Debug | LogLevel::Error => {}
            }
        }
    }

    struct TestClock {
        now: Mutex<Instant>,
    }

    impl TestClock {
        fn new(now: Instant) -> Self {
            Self {
                now: Mutex::new(now),
            }
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().expect("lock clock");
            *now = now.checked_add(duration).expect("advance clock");
        }
    }

    impl SlackProgressStreamClock for TestClock {
        fn now(&self) -> Instant {
            *self.now.lock().expect("lock clock")
        }
    }

    fn create_route() -> StartTaskProgressSessionInput {
        StartTaskProgressSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: None,
        }
    }

    fn create_chunk(text: &str) -> Vec<SlackAnyChunk> {
        vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
            text: text.to_string(),
        })]
    }

    fn create_rotation_input(
        chunks: Vec<SlackAnyChunk>,
        reason: SlackStreamRotationReason,
    ) -> SlackProgressStreamRotationInput {
        SlackProgressStreamRotationInput {
            stop_chunks: Vec::new(),
            start_chunks: chunks,
            append_chunks: None,
            reason,
        }
    }

    #[tokio::test]
    async fn restarts_stream_when_append_fails_with_message_not_in_streaming_state() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-restarted".to_string(),
        }));
        api.push_append_response(Err(PortError::service_error(
            "message_not_in_streaming_state",
            "Error: An API error occurred: message_not_in_streaming_state",
        )));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger::default());
        let mut lifecycle = SlackProgressStreamLifecycle::new(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
        );

        lifecycle.start(create_chunk("initial")).await;
        lifecycle.append(create_chunk("Collect evidence")).await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 1);
        assert_eq!(
            api.stop_calls.lock().expect("lock stop")[0].stream_ts,
            "stream-restarted"
        );
        assert!(
            logger
                .info_logs()
                .iter()
                .any(|(message, _)| message == "slack_stream_restarted")
        );
    }

    #[tokio::test]
    async fn rotates_stream_before_append_when_character_limit_would_be_exceeded() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-rotated".to_string(),
        }));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger::default());
        let mut lifecycle = SlackProgressStreamLifecycle::new(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
        );

        lifecycle.start(create_chunk(&"a".repeat(2750))).await;
        let chunks = create_chunk(&"b".repeat(200));
        let reason = lifecycle
            .rotation_reason_for_append(&chunks)
            .expect("rotation should be planned");
        lifecycle
            .rotate(create_rotation_input(chunks, reason))
            .await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 0);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 2);
        let rotation_log = logger
            .info_logs()
            .into_iter()
            .find(|(message, _)| message == "slack_stream_rotated")
            .expect("rotation log");
        assert_eq!(
            rotation_log.1.get("reason").and_then(LogFieldValue::as_str),
            Some("character_limit")
        );
    }

    #[tokio::test]
    async fn rotates_stream_when_append_fails_with_msg_too_long() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-rotated".to_string(),
        }));
        api.push_append_response(Err(PortError::service_error(
            "msg_too_long",
            "Error: An API error occurred: msg_too_long",
        )));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger::default());
        let mut lifecycle = SlackProgressStreamLifecycle::new(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
        );

        lifecycle.start(create_chunk("initial")).await;
        let chunks = create_chunk("Collect evidence");
        let outcome = lifecycle.append(chunks.clone()).await;
        assert_eq!(
            outcome,
            SlackProgressStreamAppendOutcome::RotationRequired(
                SlackStreamRotationReason::MessageTooLong
            )
        );
        lifecycle
            .rotate(create_rotation_input(
                chunks,
                SlackStreamRotationReason::MessageTooLong,
            ))
            .await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 1);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 2);
        let rotation_log = logger
            .info_logs()
            .into_iter()
            .find(|(message, _)| message == "slack_stream_rotated")
            .expect("rotation log");
        assert_eq!(
            rotation_log.1.get("reason").and_then(LogFieldValue::as_str),
            Some("msg_too_long")
        );
    }

    #[tokio::test]
    async fn rotates_stream_before_append_when_stream_age_is_near_slack_limit() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-rotated".to_string(),
        }));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger::default());
        let clock = Arc::new(TestClock::new(Instant::now()));
        let mut lifecycle = SlackProgressStreamLifecycle::new_with_clock(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
            Arc::clone(&clock) as Arc<dyn SlackProgressStreamClock>,
        );

        lifecycle.start(create_chunk("initial")).await;
        clock.advance(Duration::from_secs(290));
        let chunks = create_chunk("Collect evidence");
        let reason = lifecycle
            .rotation_reason_for_append(&chunks)
            .expect("rotation should be planned");
        lifecycle
            .rotate(create_rotation_input(chunks, reason))
            .await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 0);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 2);
        let rotation_log = logger
            .info_logs()
            .into_iter()
            .find(|(message, _)| message == "slack_stream_rotated")
            .expect("rotation log");
        assert_eq!(
            rotation_log.1.get("reason").and_then(LogFieldValue::as_str),
            Some("time_limit")
        );
    }

    #[tokio::test]
    async fn disables_stream_on_permanent_append_failure() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_append_response(Err(PortError::service_error(
            "invalid_arguments",
            "Slack API returned error: method=chat.appendStream error=invalid_arguments",
        )));
        let logger = Arc::new(MockLogger::default());
        let mut lifecycle = SlackProgressStreamLifecycle::new(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
        );

        lifecycle.start(create_chunk("initial")).await;
        lifecycle.append(create_chunk("Collect evidence")).await;
        lifecycle.stop().await;

        assert!(
            logger
                .warn_logs()
                .iter()
                .any(|(message, _)| message == "slack_progress_stream_disabled")
        );
    }

    #[tokio::test]
    async fn stop_logs_append_count_and_last_error() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_append_response(Err(PortError::service_error(
            "invalid_arguments",
            "Slack API returned error: method=chat.appendStream error=invalid_arguments",
        )));
        let logger = Arc::new(MockLogger::default());
        let mut lifecycle = SlackProgressStreamLifecycle::new(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::clone(&logger) as Arc<dyn SlackProgressStreamLogger>,
            create_route(),
        );

        lifecycle.start(create_chunk("initial")).await;
        lifecycle.append(create_chunk("Collect evidence")).await;
        lifecycle.stop().await;

        let stop_log = logger
            .info_logs()
            .into_iter()
            .find(|(message, _)| message == "slack_stream_stopped")
            .expect("stop log");
        assert_eq!(
            stop_log
                .1
                .get("slack_stream_append_count")
                .and_then(LogFieldValue::as_str),
            Some("0")
        );
        assert_eq!(
            stop_log
                .1
                .get("slack_stream_last_error")
                .and_then(LogFieldValue::as_str),
            Some("Slack API returned error: method=chat.appendStream error=invalid_arguments")
        );
    }
}
