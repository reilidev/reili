use std::sync::Arc;

use async_trait::async_trait;
use reili_core::investigation::{
    CompleteInvestigationProgressSessionInput, InvestigationProgressSessionFactoryPort,
    InvestigationProgressSessionPort, InvestigationProgressUpdate,
    StartInvestigationProgressSessionInput,
};

use super::{
    SlackProgressStreamApiPort, SlackProgressStreamLifecycle, SlackProgressStreamLogger,
    TracingSlackProgressStreamLogger, build_progress_chunks, build_stream_start_chunks,
};
use crate::outbound::slack::SlackProgressStreamAdapter;
use crate::outbound::slack::slack_web_api_client::SlackWebApiClient;

#[derive(Clone)]
pub struct SlackProgressReporter {
    api: Arc<dyn SlackProgressStreamApiPort>,
    logger: Arc<dyn SlackProgressStreamLogger>,
}

impl SlackProgressReporter {
    pub fn new(client: Arc<SlackWebApiClient>) -> Self {
        Self::new_with_dependencies(
            Arc::new(SlackProgressStreamAdapter::new(client)),
            Arc::new(TracingSlackProgressStreamLogger),
        )
    }

    pub(crate) fn new_with_dependencies(
        api: Arc<dyn SlackProgressStreamApiPort>,
        logger: Arc<dyn SlackProgressStreamLogger>,
    ) -> Self {
        Self { api, logger }
    }
}

impl InvestigationProgressSessionFactoryPort for SlackProgressReporter {
    fn create_for_thread(
        &self,
        input: StartInvestigationProgressSessionInput,
    ) -> Box<dyn InvestigationProgressSessionPort> {
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
impl InvestigationProgressSessionPort for SlackProgressReporterSession {
    async fn start(&mut self) {
        self.lifecycle.start(build_stream_start_chunks()).await;
    }

    async fn apply(&mut self, update: InvestigationProgressUpdate) {
        self.lifecycle.append(build_progress_chunks(update)).await;
    }

    async fn complete(&mut self, _input: CompleteInvestigationProgressSessionInput) {
        self.lifecycle.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reili_core::error::PortError;
    use reili_core::investigation::{
        CompleteInvestigationProgressSessionInput, InvestigationProgressSessionFactoryPort,
        InvestigationProgressUpdate, StartInvestigationProgressSessionInput,
    };

    use super::{SlackProgressReporter, SlackProgressStreamApiPort, SlackProgressStreamLogger};
    use crate::outbound::slack::progress_stream::{
        SlackAppendStreamInput, SlackProgressLogMeta, SlackStartStreamInput,
        SlackStartStreamOutput, SlackStopStreamInput,
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

    impl SlackProgressStreamLogger for MockLogger {
        fn info(&self, _message: &str, _meta: SlackProgressLogMeta) {}

        fn warn(&self, _message: &str, _meta: SlackProgressLogMeta) {}
    }

    #[tokio::test]
    async fn renders_semantic_updates_and_stops_stream() {
        let api = Arc::new(MockApi::new());
        let factory = SlackProgressReporter::new_with_dependencies(
            Arc::clone(&api) as Arc<dyn SlackProgressStreamApiPort>,
            Arc::new(MockLogger),
        );
        let mut session = factory.create_for_thread(StartInvestigationProgressSessionInput {
            channel: "C123".to_string(),
            thread_ts: "123.456".to_string(),
            recipient_user_id: "U123".to_string(),
            recipient_team_id: None,
        });

        session.start().await;
        session
            .apply(InvestigationProgressUpdate::ScopeStarted {
                step_id: "progress-step-1".to_string(),
                owner_id: "investigation_lead".to_string(),
                title: "Collect evidence".to_string(),
                detail: Some("Inspect logs\n".to_string()),
            })
            .await;
        session
            .complete(CompleteInvestigationProgressSessionInput {
                status: reili_core::investigation::InvestigationProgressSessionCompletionStatus::Succeeded,
            })
            .await;

        assert_eq!(api.start_calls.lock().expect("lock start").len(), 1);
        assert_eq!(api.append_calls.lock().expect("lock append").len(), 1);
        assert_eq!(api.stop_calls.lock().expect("lock stop").len(), 1);
    }
}
