use std::sync::Arc;

use reili_core::investigation::StartInvestigationProgressSessionInput;
use serde_json::Value;

use super::chunk_rotation::{
    STREAM_ROTATION_CHARACTER_LIMIT, count_chunk_characters, should_rotate_stream,
};
use super::{
    SlackAnyChunk, SlackAppendStreamInput, SlackProgressStreamApiPort, SlackProgressStreamLogger,
    SlackStartStreamInput, SlackStopStreamInput, string_log_meta,
};

pub(crate) struct SlackProgressStreamLifecycle {
    api: Arc<dyn SlackProgressStreamApiPort>,
    logger: Arc<dyn SlackProgressStreamLogger>,
    route: StartInvestigationProgressSessionInput,
    stream_ts: Option<String>,
    current_stream_character_count: usize,
    stream_stopped: bool,
    append_count: u64,
    last_error_message: Option<String>,
}

struct RecoverFromAppendFailureInput {
    chunks: Vec<SlackAnyChunk>,
    failed_stream_ts: String,
    error_message: String,
}

impl SlackProgressStreamLifecycle {
    pub(crate) fn new(
        api: Arc<dyn SlackProgressStreamApiPort>,
        logger: Arc<dyn SlackProgressStreamLogger>,
        route: StartInvestigationProgressSessionInput,
    ) -> Self {
        Self {
            api,
            logger,
            route,
            stream_ts: None,
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
                self.stream_ts = Some(stream.stream_ts.clone());
                self.current_stream_character_count = count_chunk_characters(&chunks);
                let mut meta = string_log_meta([
                    ("channel", self.route.channel.clone()),
                    ("threadTs", self.route.thread_ts.clone()),
                    ("streamTs", stream.stream_ts),
                ]);
                if let Some(last_error_message) = &self.last_error_message {
                    meta.insert(
                        "slack_stream_last_error".to_string(),
                        Value::String(last_error_message.clone()),
                    );
                }
                self.logger.info("slack_stream_started", meta);
            }
            Err(error) => {
                self.log_start_failure(error.message);
            }
        }
    }

    pub(crate) async fn append(&mut self, chunks: Vec<SlackAnyChunk>) {
        if self.stream_stopped || self.stream_ts.is_none() {
            return;
        }

        if should_rotate_stream(self.current_stream_character_count, &chunks) {
            self.rotate_stream_with_chunks(chunks).await;
            return;
        }

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

    async fn recover_from_append_failure(&mut self, input: RecoverFromAppendFailureInput) {
        self.last_error_message = Some(input.error_message.clone());
        self.logger.warn(
            "Failed to append Slack progress stream",
            string_log_meta([
                ("channel", self.route.channel.clone()),
                ("threadTs", self.route.thread_ts.clone()),
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

    async fn restart_stream_with_chunks(
        &mut self,
        chunks: Vec<SlackAnyChunk>,
        failed_stream_ts: Option<String>,
        error_message: String,
    ) {
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
            return;
        };

        self.stream_ts = Some(stream.stream_ts.clone());
        self.current_stream_character_count = chunk_character_count;
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
                Value::String(previous_stream_ts),
            );
        }
        self.logger.info("slack_stream_restarted", meta);
    }

    async fn rotate_stream_with_chunks(&mut self, chunks: Vec<SlackAnyChunk>) {
        let previous_stream_ts = self.stream_ts.clone();
        let previous_character_count = self.current_stream_character_count;
        let chunk_character_count = count_chunk_characters(&chunks);

        self.stop_current_stream_for_rotation().await;

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
        self.logger.info("slack_stream_rotated", meta);
    }

    async fn stop_current_stream_for_rotation(&mut self) {
        let Some(stream_ts) = self.stream_ts.clone() else {
            return;
        };

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
            meta.insert("streamTs".to_string(), Value::String(stream_ts.clone()));
        }
        if let Some(last_error_message) = &self.last_error_message {
            meta.insert(
                "slack_stream_last_error".to_string(),
                Value::String(last_error_message.clone()),
            );
        }

        self.logger.info("slack_stream_stopped", meta);
    }
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::investigation::StartInvestigationProgressSessionInput;

    use crate::outbound::slack::progress_stream::{
        SlackAnyChunk, SlackAppendStreamInput, SlackMarkdownTextChunk, SlackProgressLogMeta,
        SlackProgressStreamApiPort, SlackProgressStreamLifecycle, SlackProgressStreamLogger,
        SlackStartStreamInput, SlackStartStreamOutput, SlackStopStreamInput,
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
        info_logs: Mutex<Vec<(String, SlackProgressLogMeta)>>,
        warn_logs: Mutex<Vec<(String, SlackProgressLogMeta)>>,
    }

    impl MockLogger {
        fn info_logs(&self) -> Vec<(String, SlackProgressLogMeta)> {
            self.info_logs.lock().expect("lock info logs").clone()
        }

        fn warn_logs(&self) -> Vec<(String, SlackProgressLogMeta)> {
            self.warn_logs.lock().expect("lock warn logs").clone()
        }
    }

    impl SlackProgressStreamLogger for MockLogger {
        fn info(&self, message: &str, meta: SlackProgressLogMeta) {
            self.info_logs
                .lock()
                .expect("lock info logs")
                .push((message.to_string(), meta));
        }

        fn warn(&self, message: &str, meta: SlackProgressLogMeta) {
            self.warn_logs
                .lock()
                .expect("lock warn logs")
                .push((message.to_string(), meta));
        }
    }

    fn create_route() -> StartInvestigationProgressSessionInput {
        StartInvestigationProgressSessionInput {
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

    #[tokio::test]
    async fn restarts_stream_when_append_fails_with_message_not_in_streaming_state() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-restarted".to_string(),
        }));
        api.push_append_response(Err(PortError::new(
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

        lifecycle.start(create_chunk(&"a".repeat(2650))).await;
        lifecycle.append(create_chunk(&"b".repeat(200))).await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 0);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 2);
        assert!(
            logger
                .info_logs()
                .iter()
                .any(|(message, _)| message == "slack_stream_rotated")
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
        api.push_append_response(Err(PortError::new(
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
        lifecycle.append(create_chunk("Collect evidence")).await;
        lifecycle.stop().await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 2);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 1);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 2);
    }

    #[tokio::test]
    async fn disables_stream_on_permanent_append_failure() {
        let api = Arc::new(MockSlackProgressStreamApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-initial".to_string(),
        }));
        api.push_append_response(Err(PortError::new(
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
        api.push_append_response(Err(PortError::new(
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
                .and_then(|value: &serde_json::Value| value.as_str()),
            Some("0")
        );
        assert_eq!(
            stop_log
                .1
                .get("slack_stream_last_error")
                .and_then(|value: &serde_json::Value| value.as_str()),
            Some("Slack API returned error: method=chat.appendStream error=invalid_arguments")
        );
    }
}
