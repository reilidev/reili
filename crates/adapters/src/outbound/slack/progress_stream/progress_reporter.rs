use std::sync::Arc;

use async_trait::async_trait;
use reili_core::logger::Logger;
use reili_core::task::{
    CompleteTaskProgressSessionInput, StartTaskProgressSessionInput, TaskProgressScopeStatus,
    TaskProgressSessionFactoryPort, TaskProgressSessionPort, TaskProgressUpdate,
};

use super::stream_lifecycle::{
    SlackProgressStreamAppendOutcome, SlackProgressStreamRotationInput, SlackStreamRotationReason,
};
use super::{
    SlackAnyChunk, SlackProgressStreamApiPort, SlackProgressStreamLifecycle, build_progress_chunks,
};
use crate::outbound::slack::SlackProgressStreamAdapter;
use crate::outbound::slack::slack_web_api_client::SlackWebApiClient;

pub struct SlackProgressReporterInput {
    pub client: Arc<SlackWebApiClient>,
    pub logger: Arc<dyn Logger>,
}

pub(crate) struct SlackProgressReporterDeps {
    pub api: Arc<dyn SlackProgressStreamApiPort>,
    pub logger: Arc<dyn Logger>,
}

#[derive(Clone)]
pub struct SlackProgressReporter {
    api: Arc<dyn SlackProgressStreamApiPort>,
    logger: Arc<dyn Logger>,
}

impl SlackProgressReporter {
    pub fn new(input: SlackProgressReporterInput) -> Self {
        Self::new_with_dependencies(SlackProgressReporterDeps {
            api: Arc::new(SlackProgressStreamAdapter::new(input.client)),
            logger: input.logger,
        })
    }

    pub(crate) fn new_with_dependencies(deps: SlackProgressReporterDeps) -> Self {
        Self {
            api: deps.api,
            logger: deps.logger,
        }
    }
}

impl TaskProgressSessionFactoryPort for SlackProgressReporter {
    fn create_for_thread(
        &self,
        input: StartTaskProgressSessionInput,
    ) -> Box<dyn TaskProgressSessionPort> {
        Box::new(SlackProgressReporterSession {
            lifecycle: SlackProgressStreamLifecycle::new(
                Arc::clone(&self.api),
                Arc::clone(&self.logger),
                input,
            ),
            active_scopes: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveScope {
    owner_id: String,
    step_id: String,
    title: String,
    detail: Option<String>,
}

struct SlackProgressReporterSession {
    lifecycle: SlackProgressStreamLifecycle,
    active_scopes: Vec<ActiveScope>,
}

impl SlackProgressReporterSession {
    fn update_active_scope_state(&mut self, update: &TaskProgressUpdate) {
        match update {
            TaskProgressUpdate::ScopeStarted {
                step_id,
                owner_id,
                title,
                detail,
            }
            | TaskProgressUpdate::ScopeUpdated {
                step_id,
                owner_id,
                title,
                status: TaskProgressScopeStatus::InProgress,
                detail,
            } => {
                self.upsert_active_scope(ActiveScope {
                    owner_id: owner_id.clone(),
                    step_id: step_id.clone(),
                    title: title.clone(),
                    detail: detail.clone(),
                });
            }
            TaskProgressUpdate::ScopeUpdated {
                step_id,
                owner_id,
                status: TaskProgressScopeStatus::Complete,
                ..
            }
            | TaskProgressUpdate::ScopeCompleted {
                step_id, owner_id, ..
            } => {
                self.remove_active_scope(owner_id, step_id);
            }
        }
    }

    fn upsert_active_scope(&mut self, scope: ActiveScope) {
        self.active_scopes
            .retain(|active_scope| active_scope.owner_id != scope.owner_id);
        self.active_scopes.push(scope);
    }

    fn remove_active_scope(&mut self, owner_id: &str, step_id: &str) {
        self.active_scopes.retain(|active_scope| {
            active_scope.owner_id != owner_id || active_scope.step_id != step_id
        });
    }

    fn build_rotation_input(
        &self,
        active_scopes_before_update: &[ActiveScope],
        rendered_update_chunks: Vec<SlackAnyChunk>,
        reason: SlackStreamRotationReason,
    ) -> SlackProgressStreamRotationInput {
        let stop_chunks = active_scopes_before_update
            .iter()
            .flat_map(|active_scope| {
                build_progress_chunks(TaskProgressUpdate::ScopeCompleted {
                    step_id: active_scope.step_id.clone(),
                    owner_id: active_scope.owner_id.clone(),
                    title: active_scope.title.clone(),
                })
            })
            .collect::<Vec<_>>();
        let resume_chunks = active_scopes_before_update
            .iter()
            .filter(|active_scope| self.is_scope_active(active_scope))
            .flat_map(|active_scope| {
                build_progress_chunks(TaskProgressUpdate::ScopeUpdated {
                    step_id: active_scope.step_id.clone(),
                    owner_id: active_scope.owner_id.clone(),
                    title: active_scope.title.clone(),
                    status: TaskProgressScopeStatus::InProgress,
                    detail: active_scope.detail.clone(),
                })
            })
            .collect::<Vec<_>>();

        if resume_chunks.is_empty() {
            return SlackProgressStreamRotationInput {
                stop_chunks,
                start_chunks: rendered_update_chunks,
                append_chunks: None,
                reason,
            };
        }

        SlackProgressStreamRotationInput {
            stop_chunks,
            start_chunks: resume_chunks,
            append_chunks: Some(rendered_update_chunks),
            reason,
        }
    }

    fn is_scope_active(&self, scope: &ActiveScope) -> bool {
        self.active_scopes.iter().any(|active_scope| {
            active_scope.owner_id == scope.owner_id && active_scope.step_id == scope.step_id
        })
    }
}

#[async_trait]
impl TaskProgressSessionPort for SlackProgressReporterSession {
    async fn start(&mut self) {
        // Start lazily on the first semantic progress update so we do not post an empty
        // hourglass-only stream message.
    }

    async fn apply(&mut self, update: TaskProgressUpdate) {
        let active_scopes_before_update = self.active_scopes.clone();
        let rendered_update_chunks = build_progress_chunks(update.clone());
        self.update_active_scope_state(&update);

        if let Some(reason) = self
            .lifecycle
            .rotation_reason_for_append(&rendered_update_chunks)
        {
            self.lifecycle
                .rotate(self.build_rotation_input(
                    &active_scopes_before_update,
                    rendered_update_chunks,
                    reason,
                ))
                .await;
            return;
        }

        if let SlackProgressStreamAppendOutcome::RotationRequired(reason) =
            self.lifecycle.append(rendered_update_chunks.clone()).await
        {
            self.lifecycle
                .rotate(self.build_rotation_input(
                    &active_scopes_before_update,
                    rendered_update_chunks,
                    reason,
                ))
                .await;
        }
    }

    async fn complete(&mut self, _input: CompleteTaskProgressSessionInput) {
        self.lifecycle.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::logger::{LogEntry, Logger};
    use reili_core::task::{
        CompleteTaskProgressSessionInput, StartTaskProgressSessionInput, TaskProgressScopeStatus,
        TaskProgressSessionCompletionStatus, TaskProgressSessionFactoryPort,
        TaskProgressSessionPort, TaskProgressUpdate,
    };

    use super::{
        SlackProgressReporter, SlackProgressReporterDeps, SlackProgressReporterSession,
        SlackProgressStreamApiPort, SlackProgressStreamLifecycle,
    };
    use crate::outbound::slack::progress_stream::stream_lifecycle::SlackProgressStreamClock;
    use crate::outbound::slack::progress_stream::{
        SlackAnyChunk, SlackAppendStreamInput, SlackStartStreamInput, SlackStartStreamOutput,
        SlackStopStreamInput, SlackTaskUpdateChunk, SlackTaskUpdateStatus,
    };

    struct MockApi {
        start_calls: Mutex<Vec<SlackStartStreamInput>>,
        append_calls: Mutex<Vec<SlackAppendStreamInput>>,
        stop_calls: Mutex<Vec<SlackStopStreamInput>>,
        start_responses: Mutex<VecDeque<Result<SlackStartStreamOutput, PortError>>>,
        append_responses: Mutex<VecDeque<Result<(), PortError>>>,
        stop_responses: Mutex<VecDeque<Result<(), PortError>>>,
    }

    impl MockApi {
        fn new() -> Self {
            Self {
                start_calls: Mutex::new(Vec::new()),
                append_calls: Mutex::new(Vec::new()),
                stop_calls: Mutex::new(Vec::new()),
                start_responses: Mutex::new(VecDeque::from([Ok(SlackStartStreamOutput {
                    stream_ts: "stream-1".to_string(),
                })])),
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
    impl SlackProgressStreamApiPort for MockApi {
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
                        stream_ts: "stream-1".to_string(),
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
    struct MockLogger;

    impl Logger for MockLogger {
        fn log(&self, _entry: LogEntry) {}
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

    fn create_session(
        api: Arc<MockApi>,
        logger: Arc<MockLogger>,
    ) -> Box<dyn TaskProgressSessionPort> {
        let factory = SlackProgressReporter::new_with_dependencies(SlackProgressReporterDeps {
            api: api as Arc<dyn SlackProgressStreamApiPort>,
            logger,
        });

        factory.create_for_thread(create_route())
    }

    fn create_session_with_clock(
        api: Arc<MockApi>,
        logger: Arc<MockLogger>,
        clock: Arc<TestClock>,
    ) -> SlackProgressReporterSession {
        SlackProgressReporterSession {
            lifecycle: SlackProgressStreamLifecycle::new_with_clock(
                api as Arc<dyn SlackProgressStreamApiPort>,
                logger as Arc<dyn Logger>,
                create_route(),
                clock as Arc<dyn SlackProgressStreamClock>,
            ),
            active_scopes: Vec::new(),
        }
    }

    fn task_update_chunk(
        id: &str,
        title: &str,
        status: SlackTaskUpdateStatus,
        details: Option<&str>,
        output: Option<&str>,
    ) -> SlackAnyChunk {
        SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: id.to_string(),
            title: title.to_string(),
            status,
            details: details.map(str::to_string),
            output: output.map(str::to_string),
            sources: None,
        })
    }

    fn scope_started(step_id: &str, detail: &str) -> TaskProgressUpdate {
        TaskProgressUpdate::ScopeStarted {
            step_id: step_id.to_string(),
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            detail: Some(detail.to_string()),
        }
    }

    fn scope_updated(detail: &str) -> TaskProgressUpdate {
        TaskProgressUpdate::ScopeUpdated {
            step_id: "progress-step-1".to_string(),
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            status: TaskProgressScopeStatus::InProgress,
            detail: Some(detail.to_string()),
        }
    }

    #[tokio::test]
    async fn renders_semantic_updates_and_stops_stream() {
        let api = Arc::new(MockApi::new());
        let logger = Arc::new(MockLogger);
        let mut session = create_session(Arc::clone(&api), Arc::clone(&logger));

        session.start().await;
        assert!(api.start_calls.lock().expect("lock start").is_empty());
        session
            .apply(scope_started("progress-step-1", "Inspect logs\n"))
            .await;
        session
            .complete(CompleteTaskProgressSessionInput {
                status: TaskProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 1);
        assert!(api.append_calls.lock().expect("lock append").is_empty());
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 1);
    }

    #[tokio::test]
    async fn closes_old_scope_and_resumes_it_on_character_limit_rotation() {
        let api = Arc::new(MockApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-2".to_string(),
        }));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger);
        let mut session = create_session(Arc::clone(&api), logger);

        session.start().await;
        session
            .apply(scope_started(
                "progress-step-1",
                &format!("{}\n", "a".repeat(2650)),
            ))
            .await;
        session
            .apply(scope_updated(&format!("{}\n", "b".repeat(200))))
            .await;
        session
            .complete(CompleteTaskProgressSessionInput {
                status: TaskProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        let stop_calls = api.stop_calls.lock().expect("lock stop calls");
        assert_eq!(stop_calls.len(), 2);
        assert_eq!(
            stop_calls[0].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::Complete,
                None,
                Some("done"),
            )])
        );

        let start_calls = api.start_calls.lock().expect("lock start calls");
        assert_eq!(start_calls.len(), 2);
        assert_eq!(
            start_calls[1].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some(&format!("{}\n", "a".repeat(2650))),
                None,
            )])
        );

        let append_calls = api.append_calls.lock().expect("lock append calls");
        assert_eq!(append_calls.len(), 1);
        assert_eq!(
            append_calls[0].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some(&format!("{}\n", "b".repeat(200))),
                None,
            )])
        );
    }

    #[tokio::test]
    async fn closes_old_scope_and_resumes_it_on_msg_too_long_rotation() {
        let api = Arc::new(MockApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-2".to_string(),
        }));
        api.push_append_response(Err(PortError::service_error(
            "msg_too_long",
            "Error: An API error occurred: msg_too_long",
        )));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger);
        let mut session = create_session(Arc::clone(&api), logger);

        session.start().await;
        session
            .apply(scope_started("progress-step-1", "Inspect logs\n"))
            .await;
        session.apply(scope_updated("Read traces\n")).await;
        session
            .complete(CompleteTaskProgressSessionInput {
                status: TaskProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        let stop_calls = api.stop_calls.lock().expect("lock stop calls");
        assert_eq!(stop_calls.len(), 2);
        assert_eq!(
            stop_calls[0].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::Complete,
                None,
                Some("done"),
            )])
        );

        let start_calls = api.start_calls.lock().expect("lock start calls");
        assert_eq!(start_calls.len(), 2);
        assert_eq!(
            start_calls[1].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some("Inspect logs\n"),
                None,
            )])
        );

        let append_calls = api.append_calls.lock().expect("lock append calls");
        assert_eq!(append_calls.len(), 2);
        assert_eq!(
            append_calls[1].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some("Read traces\n"),
                None,
            )])
        );
    }

    #[tokio::test]
    async fn closes_old_scope_and_resumes_it_on_time_limit_rotation() {
        let api = Arc::new(MockApi::new());
        api.push_start_response(Ok(SlackStartStreamOutput {
            stream_ts: "stream-2".to_string(),
        }));
        api.push_stop_response(Ok(()));
        api.push_stop_response(Ok(()));
        let logger = Arc::new(MockLogger);
        let clock = Arc::new(TestClock::new(Instant::now()));
        let mut session =
            create_session_with_clock(Arc::clone(&api), Arc::clone(&logger), Arc::clone(&clock));

        session.start().await;
        session
            .apply(scope_started("progress-step-1", "Inspect logs\n"))
            .await;
        clock.advance(Duration::from_secs(290));
        session.apply(scope_updated("Read traces\n")).await;
        session
            .complete(CompleteTaskProgressSessionInput {
                status: TaskProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        let stop_calls = api.stop_calls.lock().expect("lock stop calls");
        assert_eq!(stop_calls.len(), 2);
        assert_eq!(
            stop_calls[0].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::Complete,
                None,
                Some("done"),
            )])
        );

        let start_calls = api.start_calls.lock().expect("lock start calls");
        assert_eq!(start_calls.len(), 2);
        assert_eq!(
            start_calls[1].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some("Inspect logs\n"),
                None,
            )])
        );

        let append_calls = api.append_calls.lock().expect("lock append calls");
        assert_eq!(append_calls.len(), 1);
        assert_eq!(
            append_calls[0].chunks,
            Some(vec![task_update_chunk(
                "progress-step-1",
                "Collect evidence",
                SlackTaskUpdateStatus::InProgress,
                Some("Read traces\n"),
                None,
            )])
        );
    }
}
