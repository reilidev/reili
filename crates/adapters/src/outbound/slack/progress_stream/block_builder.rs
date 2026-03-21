use reili_core::investigation::{InvestigationProgressScopeStatus, InvestigationProgressUpdate};

use super::{SlackAnyChunk, SlackMarkdownTextChunk, SlackTaskUpdateChunk, SlackTaskUpdateStatus};

const STREAM_START_TEXT: &str = ":hourglass_flowing_sand:";

pub(crate) fn build_stream_start_chunks() -> Vec<SlackAnyChunk> {
    vec![SlackAnyChunk::MarkdownText(SlackMarkdownTextChunk {
        text: STREAM_START_TEXT.to_string(),
    })]
}

pub(crate) fn build_progress_chunks(update: InvestigationProgressUpdate) -> Vec<SlackAnyChunk> {
    vec![match update {
        InvestigationProgressUpdate::ScopeStarted {
            step_id,
            title,
            detail,
            ..
        } => SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: step_id,
            title,
            status: SlackTaskUpdateStatus::InProgress,
            details: detail,
            output: None,
            sources: None,
        }),
        InvestigationProgressUpdate::ScopeUpdated {
            step_id,
            title,
            status,
            detail,
            ..
        } => SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: step_id,
            title,
            status: match status {
                InvestigationProgressScopeStatus::InProgress => SlackTaskUpdateStatus::InProgress,
                InvestigationProgressScopeStatus::Complete => SlackTaskUpdateStatus::Complete,
            },
            details: detail,
            output: matches!(status, InvestigationProgressScopeStatus::Complete)
                .then(|| "done".to_string()),
            sources: None,
        }),
        InvestigationProgressUpdate::ScopeCompleted { step_id, title, .. } => {
            SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
                id: step_id,
                title,
                status: SlackTaskUpdateStatus::Complete,
                details: None,
                output: Some("done".to_string()),
                sources: None,
            })
        }
    }]
}

#[cfg(test)]
mod tests {
    use reili_core::investigation::{
        InvestigationProgressScopeStatus, InvestigationProgressUpdate,
    };

    use super::{SlackAnyChunk, SlackTaskUpdateStatus, build_progress_chunks};

    #[test]
    fn renders_scope_started_as_in_progress_task_update() {
        let chunks = build_progress_chunks(InvestigationProgressUpdate::ScopeStarted {
            step_id: "progress-step-1".to_string(),
            owner_id: "investigation_lead".to_string(),
            title: "Collect evidence".to_string(),
            detail: Some("Inspect logs\n".to_string()),
        });

        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.id, "progress-step-1");
                assert_eq!(chunk.title, "Collect evidence");
                assert_eq!(chunk.status, SlackTaskUpdateStatus::InProgress);
                assert_eq!(chunk.details.as_deref(), Some("Inspect logs\n"));
                assert_eq!(chunk.output, None);
            }
            _ => panic!("expected task update chunk"),
        }
    }

    #[test]
    fn renders_scope_completed_as_done_task_update() {
        let chunks = build_progress_chunks(InvestigationProgressUpdate::ScopeCompleted {
            step_id: "progress-step-1".to_string(),
            owner_id: "investigation_lead".to_string(),
            title: "Collect evidence".to_string(),
        });

        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.status, SlackTaskUpdateStatus::Complete);
                assert_eq!(chunk.output.as_deref(), Some("done"));
            }
            _ => panic!("expected task update chunk"),
        }
    }

    #[test]
    fn renders_scope_updated_with_complete_status_as_done_task_update() {
        let chunks = build_progress_chunks(InvestigationProgressUpdate::ScopeUpdated {
            step_id: "progress-step-1".to_string(),
            owner_id: "investigation_lead".to_string(),
            title: "Collect evidence".to_string(),
            status: InvestigationProgressScopeStatus::Complete,
            detail: None,
        });

        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            SlackAnyChunk::TaskUpdate(chunk) => {
                assert_eq!(chunk.status, SlackTaskUpdateStatus::Complete);
                assert_eq!(chunk.output.as_deref(), Some("done"));
            }
            _ => panic!("expected task update chunk"),
        }
    }
}
