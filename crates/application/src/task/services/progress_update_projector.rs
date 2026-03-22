use reili_core::task::{TaskProgressScopeStatus, TaskProgressUpdate};

use super::progress_stream_state::{
    ProgressStep, ProgressStepStatus, ProgressStreamState, ResolveToolStartedProgressStepOutput,
    ToolCallStatus, resolve_progress_step_status,
};
use super::progress_update_commands::{
    RecordMessageOutputCreated, RecordProgressSummary, RecordToolCallCompleted,
    RecordToolCallStarted,
};

pub(super) struct ToolStartedProgressProjection {
    pub updates: Vec<TaskProgressUpdate>,
    pub resolved_progress_step: ResolveToolStartedProgressStepOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolCompletedProgressProjection {
    MissingProgressStep,
    Applied(Vec<TaskProgressUpdate>),
}

// Projects progress inputs into semantic updates while preserving the step state
// needed to interpret later events in the same task flow.
#[derive(Debug)]
pub(super) struct ProgressUpdateProjector {
    state: ProgressStreamState,
}

impl ProgressUpdateProjector {
    pub(super) fn new() -> Self {
        Self {
            state: ProgressStreamState::new(),
        }
    }

    pub(super) fn project_progress_summary(
        &mut self,
        input: RecordProgressSummary,
    ) -> Vec<TaskProgressUpdate> {
        if input.title.trim().is_empty() {
            return Vec::new();
        }

        let mut updates = Vec::new();

        if let Some(progress_step) = self
            .state
            .complete_active_progress_step_if_idle(&input.owner_id)
        {
            updates.push(scope_completed(progress_step));
        }

        let progress_step_id = self
            .state
            .create_progress_step(&input.owner_id, &input.title);
        self.state
            .set_active_progress_step(&input.owner_id, progress_step_id.clone());

        if let Some(progress_step) = self.state.progress_step(&progress_step_id) {
            updates.push(scope_started(
                progress_step,
                normalize_progress_summary(&input.summary),
            ));
        }

        updates
    }

    pub(super) fn project_tool_started(
        &mut self,
        input: RecordToolCallStarted,
    ) -> ToolStartedProgressProjection {
        let resolved = self.state.resolve_progress_step_for_tool_started(
            &input.owner_id,
            &input.task_id,
            "Tool executions",
        );
        self.state.upsert_progress_step_tool_call_status(
            &resolved.progress_step_id,
            &input.owner_id,
            &input.task_id,
            ToolCallStatus::InProgress,
        );
        self.state
            .mark_progress_step_incomplete(&resolved.progress_step_id);

        let mut updates = Vec::new();
        if let Some(progress_step) = self.state.progress_step(&resolved.progress_step_id) {
            updates.push(scope_updated(
                progress_step,
                ProgressStepStatus::InProgress,
                Some(build_tool_detail_line(&input.title)),
            ));
        }

        ToolStartedProgressProjection {
            updates,
            resolved_progress_step: resolved,
        }
    }

    pub(super) fn project_tool_completed(
        &mut self,
        input: RecordToolCallCompleted,
    ) -> ToolCompletedProgressProjection {
        let Some(progress_step_id) = self
            .state
            .resolve_progress_step_for_tool_completed(&input.owner_id, &input.task_id)
        else {
            return ToolCompletedProgressProjection::MissingProgressStep;
        };

        self.state.upsert_progress_step_tool_call_status(
            &progress_step_id,
            &input.owner_id,
            &input.task_id,
            ToolCallStatus::Complete,
        );

        let Some(progress_step) = self.state.progress_step(&progress_step_id) else {
            return ToolCompletedProgressProjection::Applied(Vec::new());
        };

        if resolve_progress_step_status(&progress_step) == ProgressStepStatus::Complete {
            return ToolCompletedProgressProjection::Applied(Vec::new());
        }

        ToolCompletedProgressProjection::Applied(vec![scope_updated(
            progress_step,
            ProgressStepStatus::InProgress,
            None,
        )])
    }

    pub(super) fn project_message_output_created(
        &mut self,
        input: RecordMessageOutputCreated,
    ) -> Vec<TaskProgressUpdate> {
        let updates = self
            .state
            .progress_step_ids_for_owner(&input.owner_id)
            .into_iter()
            .filter_map(|progress_step_id| {
                self.state
                    .mark_progress_step_completed(&progress_step_id)
                    .map(scope_completed)
            })
            .collect();

        self.state.clear_active_progress_step(&input.owner_id);
        updates
    }
}

fn scope_started(progress_step: ProgressStep, detail: Option<String>) -> TaskProgressUpdate {
    TaskProgressUpdate::ScopeStarted {
        step_id: progress_step.progress_step_id,
        owner_id: progress_step.owner_id,
        title: progress_step.title,
        detail,
    }
}

fn scope_updated(
    progress_step: ProgressStep,
    status: ProgressStepStatus,
    detail: Option<String>,
) -> TaskProgressUpdate {
    TaskProgressUpdate::ScopeUpdated {
        step_id: progress_step.progress_step_id,
        owner_id: progress_step.owner_id,
        title: progress_step.title,
        status: match status {
            ProgressStepStatus::InProgress => TaskProgressScopeStatus::InProgress,
            ProgressStepStatus::Complete => TaskProgressScopeStatus::Complete,
        },
        detail,
    }
}

fn scope_completed(progress_step: ProgressStep) -> TaskProgressUpdate {
    TaskProgressUpdate::ScopeCompleted {
        step_id: progress_step.progress_step_id,
        owner_id: progress_step.owner_id,
        title: progress_step.title,
    }
}

fn build_tool_detail_line(tool_name: &str) -> String {
    format!("{tool_name}\n")
}

fn normalize_progress_summary(summary: &str) -> Option<String> {
    let trimmed_summary = summary.trim();
    if trimmed_summary.is_empty() {
        return None;
    }

    Some(format!("{trimmed_summary}\n"))
}

#[cfg(test)]
mod tests {
    use reili_core::task::{TaskProgressScopeStatus, TaskProgressUpdate};

    use super::{
        ProgressUpdateProjector, RecordMessageOutputCreated, RecordProgressSummary,
        RecordToolCallCompleted, RecordToolCallStarted, ToolCompletedProgressProjection,
    };

    #[test]
    fn progress_summary_creates_scope_started_update() {
        let mut projector = ProgressUpdateProjector::new();

        let updates = projector.project_progress_summary(RecordProgressSummary {
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            summary: "Inspect logs".to_string(),
        });

        assert_eq!(
            updates,
            vec![TaskProgressUpdate::ScopeStarted {
                step_id: "progress-step-1".to_string(),
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                detail: Some("Inspect logs\n".to_string()),
            }]
        );
    }

    #[test]
    fn tool_started_and_completed_keep_scope_in_progress_until_output_exists() {
        let mut projector = ProgressUpdateProjector::new();
        projector.project_progress_summary(RecordProgressSummary {
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            summary: String::new(),
        });

        let started = projector.project_tool_started(RecordToolCallStarted {
            owner_id: "task_runner".to_string(),
            task_id: "task-1".to_string(),
            title: "logs".to_string(),
        });
        let completed = projector.project_tool_completed(RecordToolCallCompleted {
            owner_id: "task_runner".to_string(),
            task_id: "task-1".to_string(),
            title: "logs".to_string(),
        });

        assert_eq!(
            started.updates,
            vec![TaskProgressUpdate::ScopeUpdated {
                step_id: "progress-step-1".to_string(),
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                status: TaskProgressScopeStatus::InProgress,
                detail: Some("logs\n".to_string()),
            }]
        );
        assert_eq!(
            completed,
            ToolCompletedProgressProjection::Applied(Vec::new())
        );
    }

    #[test]
    fn message_output_created_completes_open_scopes() {
        let mut projector = ProgressUpdateProjector::new();
        projector.project_progress_summary(RecordProgressSummary {
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            summary: "Inspect logs".to_string(),
        });

        let updates = projector.project_message_output_created(RecordMessageOutputCreated {
            owner_id: "task_runner".to_string(),
        });

        assert_eq!(
            updates,
            vec![TaskProgressUpdate::ScopeCompleted {
                step_id: "progress-step-1".to_string(),
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
            }]
        );
    }

    #[test]
    fn owners_are_isolated() {
        let mut projector = ProgressUpdateProjector::new();
        projector.project_progress_summary(RecordProgressSummary {
            owner_id: "owner-1".to_string(),
            title: "Collect evidence".to_string(),
            summary: String::new(),
        });
        projector.project_progress_summary(RecordProgressSummary {
            owner_id: "owner-2".to_string(),
            title: "Check metrics".to_string(),
            summary: String::new(),
        });

        let updates = projector.project_message_output_created(RecordMessageOutputCreated {
            owner_id: "owner-1".to_string(),
        });

        assert_eq!(
            updates,
            vec![TaskProgressUpdate::ScopeCompleted {
                step_id: "progress-step-1".to_string(),
                owner_id: "owner-1".to_string(),
                title: "Collect evidence".to_string(),
            }]
        );
    }

    #[test]
    fn ignores_empty_progress_summary_titles() {
        let mut projector = ProgressUpdateProjector::new();

        let updates = projector.project_progress_summary(RecordProgressSummary {
            owner_id: "task_runner".to_string(),
            title: "   ".to_string(),
            summary: "Inspect logs".to_string(),
        });

        assert!(updates.is_empty());
    }

    #[test]
    fn reopens_latest_completed_scope_for_new_tool_activity() {
        let mut projector = ProgressUpdateProjector::new();
        projector.project_progress_summary(RecordProgressSummary {
            owner_id: "task_runner".to_string(),
            title: "Collect evidence".to_string(),
            summary: String::new(),
        });
        projector.project_message_output_created(RecordMessageOutputCreated {
            owner_id: "task_runner".to_string(),
        });

        let started = projector.project_tool_started(RecordToolCallStarted {
            owner_id: "task_runner".to_string(),
            task_id: "task-2".to_string(),
            title: "query metrics".to_string(),
        });

        assert_eq!(
            started
                .resolved_progress_step
                .reopened_from_progress_step_id,
            Some("progress-step-1".to_string())
        );
        assert_eq!(
            started.updates,
            vec![TaskProgressUpdate::ScopeUpdated {
                step_id: "progress-step-2".to_string(),
                owner_id: "task_runner".to_string(),
                title: "Collect evidence".to_string(),
                status: TaskProgressScopeStatus::InProgress,
                detail: Some("query metrics\n".to_string()),
            }]
        );
    }
}
