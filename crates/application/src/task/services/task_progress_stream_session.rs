use std::sync::Arc;

use async_trait::async_trait;
use reili_core::task::{
    CompleteTaskProgressSessionInput, StartTaskProgressSessionInput,
    TaskProgressSessionCompletionStatus,
    TaskProgressSessionFactoryPort as CoreTaskProgressSessionFactoryPort,
    TaskProgressSessionPort as CoreTaskProgressSessionPort, TaskProgressUpdate,
};

use crate::task::logger::{TaskLogger, string_log_meta};

use super::progress_update_commands::{
    RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
    RecordToolCallStarted,
};
use super::progress_update_projector::{ProgressUpdateProjector, ToolCompletedProgressProjection};

pub struct CreateTaskProgressStreamSessionFactoryInput {
    pub progress_session_factory_port: Arc<dyn CoreTaskProgressSessionFactoryPort>,
    pub logger: Arc<dyn TaskLogger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskProgressStreamSessionInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait TaskProgressStreamSession: Send {
    async fn start(&mut self);
    async fn post_progress_summary(&mut self, input: RecordProgressSummary);
    async fn post_tool_started(&mut self, input: RecordToolCallStarted);
    async fn post_tool_completed(&mut self, input: RecordToolCallCompleted);
    async fn post_message_output_created(&mut self, input: RecordMessageOutputCreated);
    async fn stop_as_succeeded(&mut self);
    async fn stop_as_failed(&mut self);
}

#[cfg_attr(test, mockall::automock)]
pub trait TaskProgressStreamSessionFactory: Send + Sync {
    fn create_for_thread(
        &self,
        input: CreateTaskProgressStreamSessionInput,
    ) -> Box<dyn TaskProgressStreamSession>;
}

#[must_use]
pub fn create_task_progress_stream_session_factory(
    input: CreateTaskProgressStreamSessionFactoryInput,
) -> impl TaskProgressStreamSessionFactory {
    TaskProgressStreamSessionFactoryImpl { input }
}

struct TaskProgressStreamSessionFactoryImpl {
    input: CreateTaskProgressStreamSessionFactoryInput,
}

impl TaskProgressStreamSessionFactory for TaskProgressStreamSessionFactoryImpl {
    fn create_for_thread(
        &self,
        input: CreateTaskProgressStreamSessionInput,
    ) -> Box<dyn TaskProgressStreamSession> {
        let progress_session = self.input.progress_session_factory_port.create_for_thread(
            StartTaskProgressSessionInput {
                channel: input.channel.clone(),
                thread_ts: input.thread_ts.clone(),
                recipient_user_id: input.recipient_user_id.clone(),
                recipient_team_id: input.recipient_team_id.clone(),
            },
        );

        Box::new(TaskProgressStreamSessionFacade::new(
            input,
            Arc::clone(&self.input.logger),
            progress_session,
        ))
    }
}

struct TaskProgressStreamSessionFacade {
    input: CreateTaskProgressStreamSessionInput,
    logger: Arc<dyn TaskLogger>,
    projector: ProgressUpdateProjector,
    progress_session: Box<dyn CoreTaskProgressSessionPort>,
}

impl TaskProgressStreamSessionFacade {
    fn new(
        input: CreateTaskProgressStreamSessionInput,
        logger: Arc<dyn TaskLogger>,
        progress_session: Box<dyn CoreTaskProgressSessionPort>,
    ) -> Self {
        Self {
            input,
            logger,
            projector: ProgressUpdateProjector::new(),
            progress_session,
        }
    }

    async fn apply_updates(&mut self, updates: Vec<TaskProgressUpdate>) {
        for update in updates {
            self.progress_session.apply(update).await;
        }
    }

    fn log_reopened_progress_step(
        &self,
        input: &RecordToolCallStarted,
        progress_step_id: &str,
        reopened_from_progress_step_id: &str,
    ) {
        self.logger.info(
            "progress_step_reopened_for_tool_started",
            string_log_meta([
                ("channel", self.input.channel.clone()),
                ("threadTs", self.input.thread_ts.clone()),
                ("ownerId", input.owner_id.clone()),
                ("taskId", input.task_id.clone()),
                ("toolName", input.title.clone()),
                ("reopenedProgressStepId", progress_step_id.to_string()),
                (
                    "reopenedFromProgressStepId",
                    reopened_from_progress_step_id.to_string(),
                ),
            ]),
        );
    }

    fn log_missing_progress_step_for_tool_completed(&self, input: &RecordToolCallCompleted) {
        self.logger.warn(
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

    async fn complete(&mut self, status: TaskProgressSessionCompletionStatus) {
        self.progress_session
            .complete(CompleteTaskProgressSessionInput { status })
            .await;
    }
}

#[async_trait]
impl TaskProgressStreamSession for TaskProgressStreamSessionFacade {
    async fn start(&mut self) {
        self.progress_session.start().await;
    }

    async fn post_progress_summary(&mut self, input: RecordProgressSummary) {
        let updates = self.projector.project_progress_summary(input);
        self.apply_updates(updates).await;
    }

    async fn post_tool_started(&mut self, input: RecordToolCallStarted) {
        let projection = self.projector.project_tool_started(input.clone());
        if let Some(reopened_from_progress_step_id) = projection
            .resolved_progress_step
            .reopened_from_progress_step_id
            .as_deref()
        {
            self.log_reopened_progress_step(
                &input,
                &projection.resolved_progress_step.progress_step_id,
                reopened_from_progress_step_id,
            );
        }

        self.apply_updates(projection.updates).await;
    }

    async fn post_tool_completed(&mut self, input: RecordToolCallCompleted) {
        match self.projector.project_tool_completed(input.clone()) {
            ToolCompletedProgressProjection::MissingProgressStep => {
                self.log_missing_progress_step_for_tool_completed(&input);
            }
            ToolCompletedProgressProjection::Applied(updates) => {
                self.apply_updates(updates).await;
            }
        }
    }

    async fn post_message_output_created(&mut self, input: RecordMessageOutputCreated) {
        let updates = self.projector.project_message_output_created(input);
        self.apply_updates(updates).await;
    }

    async fn stop_as_succeeded(&mut self) {
        self.complete(TaskProgressSessionCompletionStatus::Succeeded)
            .await;
    }

    async fn stop_as_failed(&mut self) {
        self.complete(TaskProgressSessionCompletionStatus::Failed)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mockall::Sequence;
    use reili_core::task::{
        CompleteTaskProgressSessionInput, MockTaskProgressSessionFactoryPort,
        MockTaskProgressSessionPort, StartTaskProgressSessionInput,
        TaskProgressSessionCompletionStatus, TaskProgressSessionPort, TaskProgressUpdate,
    };

    use super::{
        CreateTaskProgressStreamSessionFactoryInput, CreateTaskProgressStreamSessionInput,
        RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
        RecordToolCallStarted, TaskProgressStreamSessionFactory,
        create_task_progress_stream_session_factory,
    };
    use crate::task::logger::{TaskLogMeta, TaskLogger};
    use reili_core::logger::{LogEntry, LogLevel};

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, TaskLogMeta)>>,
        warn_logs: Mutex<Vec<(String, TaskLogMeta)>>,
    }

    impl MockLogger {
        fn info_logs(&self) -> Vec<(String, TaskLogMeta)> {
            self.info_logs.lock().expect("lock info logs").clone()
        }

        fn warn_logs(&self) -> Vec<(String, TaskLogMeta)> {
            self.warn_logs.lock().expect("lock warn logs").clone()
        }
    }

    impl TaskLogger for MockLogger {
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

    #[derive(Default)]
    struct MockCoreSessionState {
        created_inputs: Vec<StartTaskProgressSessionInput>,
        events: Vec<String>,
        updates: Vec<TaskProgressUpdate>,
        completions: Vec<TaskProgressSessionCompletionStatus>,
    }

    fn create_core_session_factory(
        state: Arc<Mutex<MockCoreSessionState>>,
        configure_session: impl FnOnce(&mut MockTaskProgressSessionPort),
    ) -> MockTaskProgressSessionFactoryPort {
        let mut session = MockTaskProgressSessionPort::new();
        configure_session(&mut session);

        let mut factory = MockTaskProgressSessionFactoryPort::new();
        factory.expect_create_for_thread().times(1).return_once(
            move |input: StartTaskProgressSessionInput| {
                state.lock().expect("lock state").created_inputs.push(input);
                Box::new(session) as Box<dyn TaskProgressSessionPort>
            },
        );
        factory
    }

    #[tokio::test]
    async fn forwards_semantic_updates_and_completion_to_core_session() {
        let state = Arc::new(Mutex::new(MockCoreSessionState::default()));
        let logger = Arc::new(MockLogger::default());
        let mut sequence = Sequence::new();
        let factory_state = Arc::clone(&state);
        let factory = create_task_progress_stream_session_factory(
            CreateTaskProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&factory_state),
                    |session: &mut MockTaskProgressSessionPort| {
                        let start_state = Arc::clone(&factory_state);
                        session
                            .expect_start()
                            .times(1)
                            .in_sequence(&mut sequence)
                            .returning(move || {
                                start_state
                                    .lock()
                                    .expect("lock state")
                                    .events
                                    .push("start".to_string());
                            });

                        let apply_state = Arc::clone(&factory_state);
                        session
                            .expect_apply()
                            .times(2)
                            .in_sequence(&mut sequence)
                            .returning(move |update: TaskProgressUpdate| {
                                let mut state = apply_state.lock().expect("lock state");
                                state.events.push("apply".to_string());
                                state.updates.push(update);
                            });

                        let complete_state = Arc::clone(&factory_state);
                        session
                            .expect_complete()
                            .times(1)
                            .in_sequence(&mut sequence)
                            .returning(move |input: CompleteTaskProgressSessionInput| {
                                let mut state = complete_state.lock().expect("lock state");
                                state.events.push("complete".to_string());
                                state.completions.push(input.status);
                            });
                    },
                )),
                logger,
            },
        );
        let mut session = factory.create_for_thread(CreateTaskProgressStreamSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: Some("T123".to_string()),
        });

        session.start().await;
        session
            .post_progress_summary(RecordProgressSummary {
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                summary: "Inspect logs".to_string(),
            })
            .await;
        session
            .post_message_output_created(RecordMessageOutputCreated {
                owner_id: "task_runner".to_string(),
            })
            .await;
        session.stop_as_succeeded().await;

        let state = state.lock().expect("lock state");
        assert_eq!(
            state.created_inputs,
            vec![StartTaskProgressSessionInput {
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: Some("T123".to_string()),
            }]
        );
        assert_eq!(
            state.events,
            vec![
                "start".to_string(),
                "apply".to_string(),
                "apply".to_string(),
                "complete".to_string(),
            ]
        );
        assert_eq!(
            state.completions,
            vec![TaskProgressSessionCompletionStatus::Succeeded]
        );
    }

    #[tokio::test]
    async fn logs_reopened_progress_step_when_new_tool_activity_reuses_completed_scope() {
        let state = Arc::new(Mutex::new(MockCoreSessionState::default()));
        let logger = Arc::new(MockLogger::default());
        let factory = create_task_progress_stream_session_factory(
            CreateTaskProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&state),
                    |session: &mut MockTaskProgressSessionPort| {
                        let apply_state = Arc::clone(&state);
                        session.expect_apply().times(3).returning(
                            move |update: TaskProgressUpdate| {
                                let mut state = apply_state.lock().expect("lock state");
                                state.events.push("apply".to_string());
                                state.updates.push(update);
                            },
                        );
                    },
                )),
                logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
            },
        );
        let mut session = factory.create_for_thread(CreateTaskProgressStreamSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: None,
        });

        session
            .post_progress_summary(RecordProgressSummary {
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                summary: String::new(),
            })
            .await;
        session
            .post_message_output_created(RecordMessageOutputCreated {
                owner_id: "task_runner".to_string(),
            })
            .await;
        session
            .post_tool_started(RecordToolCallStarted {
                owner_id: "task_runner".to_string(),
                task_id: "task-2".to_string(),
                title: "logs".to_string(),
            })
            .await;

        assert!(
            logger
                .info_logs()
                .iter()
                .any(|(message, _)| message == "progress_step_reopened_for_tool_started")
        );
    }

    #[tokio::test]
    async fn logs_when_tool_completion_arrives_without_matching_progress_step() {
        let state = Arc::new(Mutex::new(MockCoreSessionState::default()));
        let logger = Arc::new(MockLogger::default());
        let factory = create_task_progress_stream_session_factory(
            CreateTaskProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&state),
                    |session: &mut MockTaskProgressSessionPort| {
                        session.expect_apply().times(0);
                    },
                )),
                logger: Arc::clone(&logger) as Arc<dyn TaskLogger>,
            },
        );
        let mut session = factory.create_for_thread(CreateTaskProgressStreamSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: None,
        });

        session
            .post_tool_completed(RecordToolCallCompleted {
                owner_id: "task_runner".to_string(),
                task_id: "task-404".to_string(),
                title: "logs".to_string(),
            })
            .await;

        assert!(
            logger
                .warn_logs()
                .iter()
                .any(|(message, _)| message == "progress_step_not_found_for_tool_completed")
        );
    }
}
