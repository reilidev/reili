use std::sync::Arc;

use reili_core::task::{TaskProgressEvent, TaskProgressEventInput};
use tokio::sync::Mutex;

use super::progress_update_commands::{
    RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
    RecordToolCallStarted,
};
use super::task_progress_stream_session::TaskProgressStreamSession;

pub struct TaskProgressEventHandlerInput {
    pub progress_session: Arc<Mutex<Box<dyn TaskProgressStreamSession>>>,
}

pub struct TaskProgressEventHandler {
    progress_session: Arc<Mutex<Box<dyn TaskProgressStreamSession>>>,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mockall::Sequence;
    use reili_core::task::{TaskProgressEvent, TaskProgressEventInput};
    use tokio::sync::Mutex as TokioMutex;

    use super::{TaskProgressEventHandler, TaskProgressEventHandlerInput};
    use crate::task::services::{
        RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
        RecordToolCallStarted, TaskProgressStreamSession,
        task_progress_stream_session::MockTaskProgressStreamSession,
    };

    #[tokio::test]
    async fn routes_progress_events_to_session_methods() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut sequence = Sequence::new();
        let mut session = MockTaskProgressStreamSession::new();

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
            Box::new(session) as Box<dyn TaskProgressStreamSession>
        ));
        let handler = TaskProgressEventHandler::new(TaskProgressEventHandlerInput {
            progress_session: Arc::clone(&session),
        });

        handler
            .handle(TaskProgressEventInput {
                owner_id: "task_runner".to_string(),
                event: TaskProgressEvent::ProgressSummaryCreated {
                    title: "Collect evidence".to_string(),
                    summary: "Inspect logs".to_string(),
                },
            })
            .await;
        handler
            .handle(TaskProgressEventInput {
                owner_id: "task_runner".to_string(),
                event: TaskProgressEvent::ToolCallStarted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(TaskProgressEventInput {
                owner_id: "task_runner".to_string(),
                event: TaskProgressEvent::ToolCallCompleted {
                    task_id: "task-1".to_string(),
                    title: "logs".to_string(),
                },
            })
            .await;
        handler
            .handle(TaskProgressEventInput {
                owner_id: "task_runner".to_string(),
                event: TaskProgressEvent::MessageOutputCreated,
            })
            .await;

        assert_eq!(
            events.lock().expect("lock events").clone(),
            vec![
                "progress_summary:task_runner:Collect evidence:Inspect logs".to_string(),
                "tool_started:task_runner:task-1".to_string(),
                "tool_completed:task_runner:task-1".to_string(),
                "message_output:task_runner".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn ignores_empty_progress_summary() {
        let mut session = MockTaskProgressStreamSession::new();
        session.expect_post_progress_summary().times(0);
        let session = Arc::new(TokioMutex::new(
            Box::new(session) as Box<dyn TaskProgressStreamSession>
        ));
        let handler = TaskProgressEventHandler::new(TaskProgressEventHandlerInput {
            progress_session: Arc::clone(&session),
        });

        handler
            .handle(TaskProgressEventInput {
                owner_id: "task_runner".to_string(),
                event: TaskProgressEvent::ProgressSummaryCreated {
                    title: "  ".to_string(),
                    summary: "Inspect logs".to_string(),
                },
            })
            .await;
    }
}

impl TaskProgressEventHandler {
    pub fn new(input: TaskProgressEventHandlerInput) -> Self {
        Self {
            progress_session: input.progress_session,
        }
    }

    pub async fn handle(&self, input: TaskProgressEventInput) {
        match input.event {
            TaskProgressEvent::ProgressSummaryCreated { title, summary } => {
                self.post_progress_summary(RecordProgressSummary {
                    owner_id: input.owner_id,
                    title,
                    summary,
                })
                .await;
            }
            TaskProgressEvent::ToolCallStarted { task_id, title } => {
                self.post_tool_started(RecordToolCallStarted {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            TaskProgressEvent::ToolCallCompleted { task_id, title } => {
                self.post_tool_completed(RecordToolCallCompleted {
                    owner_id: input.owner_id,
                    task_id,
                    title,
                })
                .await;
            }
            TaskProgressEvent::MessageOutputCreated => {
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
