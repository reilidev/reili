use std::sync::Arc;

use async_trait::async_trait;
use reili_core::logger::Logger;
use reili_core::task::{
    CompleteTaskProgressSessionInput, StartTaskProgressSessionInput,
    TaskProgressSessionFactoryPort, TaskProgressSessionPort, TaskProgressUpdate,
};

use super::{
    SlackProgressStreamApiPort, SlackProgressStreamLifecycle, build_progress_chunks,
    build_stream_start_chunks,
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
        })
    }
}

struct SlackProgressReporterSession {
    lifecycle: SlackProgressStreamLifecycle,
}

#[async_trait]
impl TaskProgressSessionPort for SlackProgressReporterSession {
    async fn start(&mut self) {
        self.lifecycle.start(build_stream_start_chunks()).await;
    }

    async fn apply(&mut self, update: TaskProgressUpdate) {
        self.lifecycle.append(build_progress_chunks(update)).await;
    }

    async fn complete(&mut self, _input: CompleteTaskProgressSessionInput) {
        self.lifecycle.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::logger::{LogEntry, Logger};
    use reili_core::task::{
        CompleteTaskProgressSessionInput, StartTaskProgressSessionInput,
        TaskProgressSessionFactoryPort, TaskProgressUpdate,
    };

    use super::{SlackProgressReporter, SlackProgressReporterDeps, SlackProgressStreamApiPort};
    use crate::outbound::slack::progress_stream::{
        SlackAppendStreamInput, SlackStartStreamInput, SlackStartStreamOutput, SlackStopStreamInput,
    };

    struct MockApi {
        start_calls: Mutex<Vec<SlackStartStreamInput>>,
        append_calls: Mutex<Vec<SlackAppendStreamInput>>,
        stop_calls: Mutex<Vec<SlackStopStreamInput>>,
        start_responses: Mutex<VecDeque<Result<SlackStartStreamOutput, PortError>>>,
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
            }
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
            Ok(())
        }

        async fn stop(&self, input: SlackStopStreamInput) -> Result<(), PortError> {
            self.stop_calls.lock().expect("lock stop calls").push(input);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockLogger;

    impl Logger for MockLogger {
        fn log(&self, _entry: LogEntry) {}
    }

    #[tokio::test]
    async fn renders_semantic_updates_and_stops_stream() {
        let api = Arc::new(MockApi::new());
        let factory = SlackProgressReporter::new_with_dependencies(SlackProgressReporterDeps {
            api: Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            logger: Arc::new(MockLogger),
        });
        let mut session = factory.create_for_thread(StartTaskProgressSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: None,
        });

        session.start().await;
        session
            .apply(TaskProgressUpdate::ScopeStarted {
                step_id: "progress-step-1".to_string(),
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                detail: Some("Inspect logs\n".to_string()),
            })
            .await;
        session
            .complete(CompleteTaskProgressSessionInput {
                status: reili_core::task::TaskProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 1);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 1);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 1);
    }
}
