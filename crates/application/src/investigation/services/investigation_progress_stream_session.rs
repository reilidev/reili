use std::sync::Arc;

use async_trait::async_trait;
use reili_core::investigation::{
    CompleteInvestigationProgressSessionInput, InvestigationProgressSessionCompletionStatus,
    InvestigationProgressSessionFactoryPort as CoreInvestigationProgressSessionFactoryPort,
    InvestigationProgressSessionPort as CoreInvestigationProgressSessionPort,
    InvestigationProgressUpdate, StartInvestigationProgressSessionInput,
};

use crate::investigation::logger::{InvestigationLogger, string_log_meta};

use super::progress_update_commands::{
    RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
    RecordToolCallStarted,
};
use super::progress_update_projector::{ProgressUpdateProjector, ToolCompletedProgressProjection};

pub struct CreateInvestigationProgressStreamSessionFactoryInput {
    pub progress_session_factory_port: Arc<dyn CoreInvestigationProgressSessionFactoryPort>,
    pub logger: Arc<dyn InvestigationLogger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInvestigationProgressStreamSessionInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait InvestigationProgressStreamSession: Send {
    async fn start(&mut self);
    async fn post_progress_summary(&mut self, input: RecordProgressSummary);
    async fn post_tool_started(&mut self, input: RecordToolCallStarted);
    async fn post_tool_completed(&mut self, input: RecordToolCallCompleted);
    async fn post_message_output_created(&mut self, input: RecordMessageOutputCreated);
    async fn stop_as_succeeded(&mut self);
    async fn stop_as_failed(&mut self);
}

#[cfg_attr(test, mockall::automock)]
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
    InvestigationProgressStreamSessionFactoryImpl { input }
}

struct InvestigationProgressStreamSessionFactoryImpl {
    input: CreateInvestigationProgressStreamSessionFactoryInput,
}

impl InvestigationProgressStreamSessionFactory for InvestigationProgressStreamSessionFactoryImpl {
    fn create_for_thread(
        &self,
        input: CreateInvestigationProgressStreamSessionInput,
    ) -> Box<dyn InvestigationProgressStreamSession> {
        let progress_session = self.input.progress_session_factory_port.create_for_thread(
            StartInvestigationProgressSessionInput {
                channel: input.channel.clone(),
                thread_ts: input.thread_ts.clone(),
                recipient_user_id: input.recipient_user_id.clone(),
                recipient_team_id: input.recipient_team_id.clone(),
            },
        );

        Box::new(InvestigationProgressStreamSessionFacade::new(
            input,
            Arc::clone(&self.input.logger),
            progress_session,
        ))
    }
}

struct InvestigationProgressStreamSessionFacade {
    input: CreateInvestigationProgressStreamSessionInput,
    logger: Arc<dyn InvestigationLogger>,
    projector: ProgressUpdateProjector,
    progress_session: Box<dyn CoreInvestigationProgressSessionPort>,
}

impl InvestigationProgressStreamSessionFacade {
    fn new(
        input: CreateInvestigationProgressStreamSessionInput,
        logger: Arc<dyn InvestigationLogger>,
        progress_session: Box<dyn CoreInvestigationProgressSessionPort>,
    ) -> Self {
        Self {
            input,
            logger,
            projector: ProgressUpdateProjector::new(),
            progress_session,
        }
    }

    async fn apply_updates(&mut self, updates: Vec<InvestigationProgressUpdate>) {
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

    async fn complete(&mut self, status: InvestigationProgressSessionCompletionStatus) {
        self.progress_session
            .complete(CompleteInvestigationProgressSessionInput { status })
            .await;
    }
}

#[async_trait]
impl InvestigationProgressStreamSession for InvestigationProgressStreamSessionFacade {
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
        self.complete(InvestigationProgressSessionCompletionStatus::Succeeded)
            .await;
    }

    async fn stop_as_failed(&mut self) {
        self.complete(InvestigationProgressSessionCompletionStatus::Failed)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mockall::Sequence;
    use reili_core::investigation::{
        CompleteInvestigationProgressSessionInput, InvestigationProgressSessionCompletionStatus,
        InvestigationProgressSessionPort, InvestigationProgressUpdate,
        MockInvestigationProgressSessionFactoryPort, MockInvestigationProgressSessionPort,
        StartInvestigationProgressSessionInput,
    };

    use super::{
        CreateInvestigationProgressStreamSessionFactoryInput,
        CreateInvestigationProgressStreamSessionInput, InvestigationProgressStreamSessionFactory,
        RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
        RecordToolCallStarted, create_investigation_progress_stream_session_factory,
    };
    use crate::investigation::logger::{InvestigationLogMeta, InvestigationLogger};
    use reili_core::logger::{LogEntry, LogLevel};

    #[derive(Default)]
    struct MockLogger {
        info_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
        warn_logs: Mutex<Vec<(String, InvestigationLogMeta)>>,
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
        created_inputs: Vec<StartInvestigationProgressSessionInput>,
        events: Vec<String>,
        updates: Vec<InvestigationProgressUpdate>,
        completions: Vec<InvestigationProgressSessionCompletionStatus>,
    }

    fn create_core_session_factory(
        state: Arc<Mutex<MockCoreSessionState>>,
        configure_session: impl FnOnce(&mut MockInvestigationProgressSessionPort),
    ) -> MockInvestigationProgressSessionFactoryPort {
        let mut session = MockInvestigationProgressSessionPort::new();
        configure_session(&mut session);

        let mut factory = MockInvestigationProgressSessionFactoryPort::new();
        factory.expect_create_for_thread().times(1).return_once(
            move |input: StartInvestigationProgressSessionInput| {
                state.lock().expect("lock state").created_inputs.push(input);
                Box::new(session) as Box<dyn InvestigationProgressSessionPort>
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
        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&factory_state),
                    |session: &mut MockInvestigationProgressSessionPort| {
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
                            .returning(move |update: InvestigationProgressUpdate| {
                                let mut state = apply_state.lock().expect("lock state");
                                state.events.push("apply".to_string());
                                state.updates.push(update);
                            });

                        let complete_state = Arc::clone(&factory_state);
                        session
                            .expect_complete()
                            .times(1)
                            .in_sequence(&mut sequence)
                            .returning(move |input: CompleteInvestigationProgressSessionInput| {
                                let mut state = complete_state.lock().expect("lock state");
                                state.events.push("complete".to_string());
                                state.completions.push(input.status);
                            });
                    },
                )),
                logger,
            },
        );
        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: Some("T123".to_string()),
            });

        session.start().await;
        session
            .post_progress_summary(RecordProgressSummary {
                owner_id: "investigation_lead".to_string(),
                title: "Collect evidence".to_string(),
                summary: "Inspect logs".to_string(),
            })
            .await;
        session
            .post_message_output_created(RecordMessageOutputCreated {
                owner_id: "investigation_lead".to_string(),
            })
            .await;
        session.stop_as_succeeded().await;

        let state = state.lock().expect("lock state");
        assert_eq!(
            state.created_inputs,
            vec![StartInvestigationProgressSessionInput {
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
            vec![InvestigationProgressSessionCompletionStatus::Succeeded]
        );
    }

    #[tokio::test]
    async fn logs_reopened_progress_step_when_new_tool_activity_reuses_completed_scope() {
        let state = Arc::new(Mutex::new(MockCoreSessionState::default()));
        let logger = Arc::new(MockLogger::default());
        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&state),
                    |session: &mut MockInvestigationProgressSessionPort| {
                        let apply_state = Arc::clone(&state);
                        session.expect_apply().times(3).returning(
                            move |update: InvestigationProgressUpdate| {
                                let mut state = apply_state.lock().expect("lock state");
                                state.events.push("apply".to_string());
                                state.updates.push(update);
                            },
                        );
                    },
                )),
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );
        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: None,
            });

        session
            .post_progress_summary(RecordProgressSummary {
                owner_id: "investigation_lead".to_string(),
                title: "Collect evidence".to_string(),
                summary: String::new(),
            })
            .await;
        session
            .post_message_output_created(RecordMessageOutputCreated {
                owner_id: "investigation_lead".to_string(),
            })
            .await;
        session
            .post_tool_started(RecordToolCallStarted {
                owner_id: "investigation_lead".to_string(),
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
        let factory = create_investigation_progress_stream_session_factory(
            CreateInvestigationProgressStreamSessionFactoryInput {
                progress_session_factory_port: Arc::new(create_core_session_factory(
                    Arc::clone(&state),
                    |session: &mut MockInvestigationProgressSessionPort| {
                        session.expect_apply().times(0);
                    },
                )),
                logger: Arc::clone(&logger) as Arc<dyn InvestigationLogger>,
            },
        );
        let mut session =
            factory.create_for_thread(CreateInvestigationProgressStreamSessionInput {
                channel: "C123".to_string(),
                thread_ts: "123.456".to_string(),
                recipient_user_id: "U123".to_string(),
                recipient_team_id: None,
            });

        session
            .post_tool_completed(RecordToolCallCompleted {
                owner_id: "investigation_lead".to_string(),
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
