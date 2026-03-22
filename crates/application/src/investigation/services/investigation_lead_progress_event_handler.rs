use std::sync::Arc;

use reili_core::investigation::{InvestigationProgressEvent, InvestigationProgressEventInput};
use tokio::sync::Mutex;

use super::investigation_progress_stream_session::InvestigationProgressStreamSession;
use super::progress_update_commands::{
    RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
    RecordToolCallStarted,
};

pub struct InvestigationLeadProgressEventHandlerInput {
    pub progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>>,
}

pub struct InvestigationLeadProgressEventHandler {
    progress_session: Arc<Mutex<Box<dyn InvestigationProgressStreamSession>>>,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mockall::Sequence;
    use reili_core::investigation::{InvestigationProgressEvent, InvestigationProgressEventInput};
    use tokio::sync::Mutex as TokioMutex;

    use super::{
        InvestigationLeadProgressEventHandler, InvestigationLeadProgressEventHandlerInput,
    };
    use crate::investigation::services::{
        InvestigationProgressStreamSession, RecordMessageOutputCreated, RecordProgressSummary,
        RecordToolCallCompleted, RecordToolCallStarted,
        investigation_progress_stream_session::MockInvestigationProgressStreamSession,
    };

    #[tokio::test]
    async fn routes_progress_events_to_session_methods() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut sequence = Sequence::new();
        let mut session = MockInvestigationProgressStreamSession::new();

        let progress_summary_events = Arc::clone(&events);
        session
            .expect_post_progress_summary()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |input: RecordProgressSummary| {
                progress_summary_events
                    .lock()
                    .expect("lock events")
                    .push(format!(
                        "progress_summary:{}:{}:{}",
                        input.owner_id, input.title, input.summary
                    ));
            });

        let tool_started_events = Arc::clone(&events);
        session
            .expect_post_tool_started()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |input: RecordToolCallStarted| {
                tool_started_events
                    .lock()
                    .expect("lock events")
                    .push(format!("tool_started:{}:{}", input.owner_id, input.task_id));
            });

        let tool_completed_events = Arc::clone(&events);
        session
            .expect_post_tool_completed()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |input: RecordToolCallCompleted| {
                tool_completed_events
                    .lock()
                    .expect("lock events")
                    .push(format!(
                        "tool_completed:{}:{}",
                        input.owner_id, input.task_id
                    ));
            });

        let message_output_events = Arc::clone(&events);
        session
            .expect_post_message_output_created()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(move |input: RecordMessageOutputCreated| {
                message_output_events
                    .lock()
                    .expect("lock events")
                    .push(format!("message_output:{}", input.owner_id));
            });

        let session = Arc::new(TokioMutex::new(
            Box::new(session) as Box<dyn InvestigationProgressStreamSession>
        ));
        let handler = InvestigationLeadProgressEventHandler::new(
            InvestigationLeadProgressEventHandlerInput {
                progress_session: Arc::clone(&session),
            },
        );

        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "investigation_lead".to_string(),
                event: InvestigationProgressEvent::ProgressSummaryCreated {
                    title: "Collect evidence".to_string(),
                    summary: "Inspect logs".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "investigation_lead".to_string(),
                event: InvestigationProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "investigation_lead".to_string(),
                event: InvestigationProgressEvent::ToolCallCompleted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "investigation_lead".to_string(),
                event: InvestigationProgressEvent::MessageOutputCreated,
            })
            .await;

        assert_eq!(
            events.lock().expect("lock events").clone(),
            vec![
                "progress_summary:investigation_lead:Collect evidence:Inspect logs".to_string(),
                "tool_started:investigation_lead:task-1".to_string(),
                "tool_completed:investigation_lead:task-1".to_string(),
                "message_output:investigation_lead".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn ignores_empty_progress_summary() {
        let mut session = MockInvestigationProgressStreamSession::new();
        session.expect_post_progress_summary().times(0);
        let session = Arc::new(TokioMutex::new(
            Box::new(session) as Box<dyn InvestigationProgressStreamSession>
        ));
        let handler = InvestigationLeadProgressEventHandler::new(
            InvestigationLeadProgressEventHandlerInput {
                progress_session: Arc::clone(&session),
            },
        );

        handler
            .handle(InvestigationProgressEventInput {
                owner_id: "investigation_lead".to_string(),
                event: InvestigationProgressEvent::ProgressSummaryCreated {
                    title: "  ".to_string(),
                    summary: "Inspect logs".to_string(),
                },
            })
            .await;
    }
}

impl InvestigationLeadProgressEventHandler {
    pub fn new(input: InvestigationLeadProgressEventHandlerInput) -> Self {
        Self {
            progress_session: input.progress_session,
        }
    }

    pub async fn handle(&self, input: InvestigationProgressEventInput) {
        match input.event {
            InvestigationProgressEvent::ProgressSummaryCreated { title, summary } => {
                self.post_progress_summary(RecordProgressSummary {
                    owner_id: input.owner_id,
                    title,
                    summary,
                })
                .await;
            }
            InvestigationProgressEvent::ToolCallStarted { task_id, title } => {
                self.post_tool_started(RecordToolCallStarted {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            InvestigationProgressEvent::ToolCallCompleted { task_id, title } => {
                self.post_tool_completed(RecordToolCallCompleted {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            InvestigationProgressEvent::MessageOutputCreated => {
                self.post_message_output_created(RecordMessageOutputCreated {
                    owner_id: input.owner_id,
                })
                .await;
            }
        }
    }

    async fn post_progress_summary(&self, input: RecordProgressSummary) {
        if input.title.trim().is_empty() {
            return;
        }

        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_progress_summary(input).await;
    }

    async fn post_tool_started(&self, input: RecordToolCallStarted) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_tool_started(input).await;
    }

    async fn post_tool_completed(&self, input: RecordToolCallCompleted) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_tool_completed(input).await;
    }

    async fn post_message_output_created(&self, input: RecordMessageOutputCreated) {
        let mut progress_session = self.progress_session.lock().await;
        progress_session.post_message_output_created(input).await;
    }
}
