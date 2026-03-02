use std::sync::Arc;

use sre_shared::ports::outbound::{InvestigationProgressEvent, InvestigationProgressEventInput};
use tokio::sync::Mutex;

use super::investigation_progress_stream_session::{
    InvestigationProgressMessageOutputCreatedInput, InvestigationProgressReasoningInput,
    InvestigationProgressStreamSession, InvestigationProgressTaskUpdateInput,
};

pub struct CoordinatorProgressEventHandlerInput {
    pub progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>>,
}

pub struct CoordinatorProgressEventHandler {
    progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>>,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use sre_shared::ports::outbound::{
        InvestigationProgressEvent, InvestigationProgressEventInput,
    };
    use tokio::sync::Mutex as TokioMutex;

    use super::{CoordinatorProgressEventHandler, CoordinatorProgressEventHandlerInput};
    use crate::investigation::services::investigation_progress_stream_session::{
        InvestigationProgressMessageOutputCreatedInput, InvestigationProgressReasoningInput,
        InvestigationProgressStreamSession, InvestigationProgressTaskUpdateInput,
    };

    struct SessionMock {
        events: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl InvestigationProgressStreamSession for SessionMock {
        async fn start(&mut self) {}

        async fn post_reasoning(&mut self, input: InvestigationProgressReasoningInput) {
            self.events.lock().expect("lock events").push(format!(
                "reasoning:{}:{}",
                input.owner_id, input.summary_text
            ));
        }

        async fn post_tool_started(&mut self, input: InvestigationProgressTaskUpdateInput) {
            self.events
                .lock()
                .expect("lock events")
                .push(format!("tool_started:{}:{}", input.owner_id, input.task_id));
        }

        async fn post_tool_completed(&mut self, input: InvestigationProgressTaskUpdateInput) {
            self.events.lock().expect("lock events").push(format!(
                "tool_completed:{}:{}",
                input.owner_id, input.task_id
            ));
        }

        async fn post_message_output_created(
            &mut self,
            input: InvestigationProgressMessageOutputCreatedInput,
        ) {
            self.events
                .lock()
                .expect("lock events")
                .push(format!("message_output:{}", input.owner_id));
        }

        async fn stop_as_succeeded(&mut self) {}

        async fn stop_as_failed(&mut self) {}
    }

    #[tokio::test]
    async fn routes_progress_events_to_session_methods() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let session = Arc::new(TokioMutex::new(Box::new(SessionMock {
            events: Arc::clone(&events),
        })
            as Box<dyn InvestigationProgressStreamSession>));
        let handler = CoordinatorProgressEventHandler::new(CoordinatorProgressEventHandlerInput {
            progress_session: Arc::clone(&session),
        });

        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::ReasoningSummaryCreated {
                    summary_text: "Collect evidence".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::ToolCallCompleted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::MessageOutputCreated,
            })
            .await;

        assert_eq!(
            events.lock().expect("lock events").clone(),
            vec![
                "reasoning:coordinator:Collect evidence".to_string(),
                "tool_started:coordinator:task-1".to_string(),
                "tool_completed:coordinator:task-1".to_string(),
                "message_output:coordinator".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn ignores_empty_reasoning_summary() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let session = Arc::new(TokioMutex::new(Box::new(SessionMock {
            events: Arc::clone(&events),
        })
            as Box<dyn InvestigationProgressStreamSession>));
        let handler = CoordinatorProgressEventHandler::new(CoordinatorProgressEventHandlerInput {
            progress_session: Arc::clone(&session),
        });

        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "coordinator".to_string(),
                event: InvestigationProgressEvent::ReasoningSummaryCreated {
                    summary_text: "  ".to_string(),
                },
            })
            .await;

        assert!(events.lock().expect("lock events").is_empty());
    }
}

impl CoordinatorProgressEventHandler {
    #[must_use]
    pub fn new(input: CoordinatorProgressEventHandlerInput) -> Self {
        Self {
            progress_session: input.progress_session,
        }
    }

    pub async fn handle(&self, input: InvestigationProgressEventInput) {
        match input.event {
            InvestigationProgressEvent::ReasoningSummaryCreated { summary_text } => {
                self.post_reasoning(InvestigationProgressReasoningInput {
                    owner_id: input.owner_id,
                    summary_text,
                })
                .await;
            }
            InvestigationProgressEvent::ToolCallStarted { task_id, title } => {
                self.post_tool_started(InvestigationProgressTaskUpdateInput {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            InvestigationProgressEvent::ToolCallCompleted { task_id, title } => {
                self.post_tool_completed(InvestigationProgressTaskUpdateInput {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            InvestigationProgressEvent::MessageOutputCreated => {
                self.post_message_output_created(InvestigationProgressMessageOutputCreatedInput {
                    owner_id: input.owner_id,
                })
                .await;
            }
        }
    }

    async fn post_reasoning(&self, input: InvestigationProgressReasoningInput) {
        if input.summary_text.trim().is_empty() {
            return;
        }

        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_reasoning(input).await;
    }

    async fn post_tool_started(&self, input: InvestigationProgressTaskUpdateInput) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_tool_started(input).await;
    }

    async fn post_tool_completed(&self, input: InvestigationProgressTaskUpdateInput) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_tool_completed(input).await;
    }

    async fn post_message_output_created(
        &self,
        input: InvestigationProgressMessageOutputCreatedInput,
    ) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_message_output_created(input).await;
    }
}
