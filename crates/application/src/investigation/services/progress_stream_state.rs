use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ProgressStepStatus {
    InProgress,
    Complete,
}

pub(super) type ToolCallStatus = ProgressStepStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProgressStepLifecycle {
    Active,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProgressStep {
    pub progress_step_id: String,
    pub owner_id: String,
    pub title: String,
    sequence_number: u64,
    lifecycle: ProgressStepLifecycle,
    pub tool_call_status_by_task_id: HashMap<String, ToolCallStatus>,
}

impl ProgressStep {
    fn is_completed(&self) -> bool {
        self.lifecycle == ProgressStepLifecycle::Completed
    }

    fn mark_completed(&mut self) -> bool {
        if self.is_completed() {
            return false;
        }

        self.lifecycle = ProgressStepLifecycle::Completed;
        true
    }

    fn reopen(&mut self) {
        self.lifecycle = ProgressStepLifecycle::Active;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolveToolStartedProgressStepOutput {
    pub progress_step_id: String,
    pub reopened_from_progress_step_id: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct ProgressStreamState {
    active_progress_step_id_by_owner_id: HashMap<String, String>,
    progress_step_by_id: HashMap<String, ProgressStep>,
    progress_step_id_by_task_key: HashMap<String, String>,
    next_progress_step_number: u64,
}

impl ProgressStreamState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn create_progress_step(&mut self, owner_id: &str, title: &str) -> String {
        let (progress_step_id, sequence_number) = self.create_progress_step_identifier();
        let progress_step = ProgressStep {
            progress_step_id: progress_step_id.clone(),
            owner_id: owner_id.to_string(),
            title: title.to_string(),
            sequence_number,
            lifecycle: ProgressStepLifecycle::Active,
            tool_call_status_by_task_id: HashMap::new(),
        };
        self.progress_step_by_id
            .insert(progress_step_id.clone(), progress_step);
        progress_step_id
    }

    pub(super) fn set_active_progress_step(&mut self, owner_id: &str, progress_step_id: String) {
        self.active_progress_step_id_by_owner_id
            .insert(owner_id.to_string(), progress_step_id);
    }

    pub(super) fn clear_active_progress_step(&mut self, owner_id: &str) {
        self.active_progress_step_id_by_owner_id.remove(owner_id);
    }

    pub(super) fn resolve_progress_step_for_tool_started(
        &mut self,
        owner_id: &str,
        task_id: &str,
        reopened_progress_step_default_title: &str,
    ) -> ResolveToolStartedProgressStepOutput {
        let task_ownership_key = build_task_ownership_key(owner_id, task_id);
        if let Some(existing_progress_step_id) = self
            .progress_step_id_by_task_key
            .get(&task_ownership_key)
            .filter(|progress_step_id| self.progress_step_by_id.contains_key(*progress_step_id))
        {
            return ResolveToolStartedProgressStepOutput {
                progress_step_id: existing_progress_step_id.clone(),
                reopened_from_progress_step_id: None,
            };
        }

        if let Some(active_progress_step_id) = self
            .active_progress_step_id_by_owner_id
            .get(owner_id)
            .cloned()
        {
            if self
                .progress_step_by_id
                .contains_key(&active_progress_step_id)
            {
                self.progress_step_id_by_task_key
                    .insert(task_ownership_key, active_progress_step_id.clone());
                return ResolveToolStartedProgressStepOutput {
                    progress_step_id: active_progress_step_id,
                    reopened_from_progress_step_id: None,
                };
            }

            self.active_progress_step_id_by_owner_id.remove(owner_id);
        }

        let last_completed_progress_step =
            self.resolve_latest_completed_progress_step_by_owner_id(owner_id);
        let reopened_progress_step_title = last_completed_progress_step.as_ref().map_or_else(
            || reopened_progress_step_default_title.to_string(),
            |progress_step| progress_step.title.clone(),
        );
        let reopened_progress_step_id =
            self.create_progress_step(owner_id, &reopened_progress_step_title);
        self.set_active_progress_step(owner_id, reopened_progress_step_id.clone());

        ResolveToolStartedProgressStepOutput {
            progress_step_id: reopened_progress_step_id,
            reopened_from_progress_step_id: last_completed_progress_step
                .map(|progress_step| progress_step.progress_step_id),
        }
    }

    pub(super) fn resolve_progress_step_for_tool_completed(
        &self,
        owner_id: &str,
        task_id: &str,
    ) -> Option<String> {
        let task_ownership_key = build_task_ownership_key(owner_id, task_id);
        self.progress_step_id_by_task_key
            .get(&task_ownership_key)
            .filter(|progress_step_id| self.progress_step_by_id.contains_key(*progress_step_id))
            .cloned()
    }

    pub(super) fn upsert_progress_step_tool_call_status(
        &mut self,
        progress_step_id: &str,
        owner_id: &str,
        task_id: &str,
        status: ToolCallStatus,
    ) {
        if let Some(progress_step) = self.progress_step_by_id.get_mut(progress_step_id) {
            progress_step
                .tool_call_status_by_task_id
                .insert(task_id.to_string(), status);
        }

        self.progress_step_id_by_task_key.insert(
            build_task_ownership_key(owner_id, task_id),
            progress_step_id.to_string(),
        );
    }

    pub(super) fn mark_progress_step_incomplete(&mut self, progress_step_id: &str) {
        let Some(progress_step) = self.progress_step_by_id.get_mut(progress_step_id) else {
            return;
        };

        progress_step.reopen();
    }

    pub(super) fn complete_active_progress_step_if_idle(
        &mut self,
        owner_id: &str,
    ) -> Option<ProgressStep> {
        let active_progress_step_id = self
            .active_progress_step_id_by_owner_id
            .get(owner_id)
            .cloned()?;
        let active_progress_step = match self
            .progress_step_by_id
            .get(&active_progress_step_id)
            .cloned()
        {
            Some(progress_step) => progress_step,
            None => {
                self.active_progress_step_id_by_owner_id.remove(owner_id);
                return None;
            }
        };

        if progress_step_has_in_progress_tool_call(&active_progress_step) {
            return None;
        }

        self.mark_progress_step_completed(&active_progress_step_id)
    }

    pub(super) fn mark_progress_step_completed(
        &mut self,
        progress_step_id: &str,
    ) -> Option<ProgressStep> {
        let progress_step = self.progress_step_by_id.get_mut(progress_step_id)?;
        if !progress_step.mark_completed() {
            return None;
        }
        Some(progress_step.clone())
    }

    pub(super) fn progress_step(&self, progress_step_id: &str) -> Option<ProgressStep> {
        self.progress_step_by_id.get(progress_step_id).cloned()
    }

    pub(super) fn progress_step_ids_for_owner(&self, owner_id: &str) -> Vec<String> {
        self.progress_step_by_id
            .values()
            .filter(|progress_step| progress_step.owner_id == owner_id)
            .map(|progress_step| progress_step.progress_step_id.clone())
            .collect()
    }

    fn create_progress_step_identifier(&mut self) -> (String, u64) {
        self.next_progress_step_number = self.next_progress_step_number.saturating_add(1);
        let mut progress_step_number = self.next_progress_step_number;
        let mut progress_step_id = format!("progress-step-{progress_step_number}");
        while self.progress_step_by_id.contains_key(&progress_step_id) {
            self.next_progress_step_number = self.next_progress_step_number.saturating_add(1);
            progress_step_number = self.next_progress_step_number;
            progress_step_id = format!("progress-step-{progress_step_number}");
        }

        (progress_step_id, progress_step_number)
    }

    fn resolve_latest_completed_progress_step_by_owner_id(
        &self,
        owner_id: &str,
    ) -> Option<ProgressStep> {
        self.progress_step_by_id
            .values()
            .filter(|progress_step| {
                progress_step.owner_id == owner_id && progress_step.is_completed()
            })
            .max_by_key(|progress_step| progress_step.sequence_number)
            .cloned()
    }
}

pub(super) fn progress_step_has_in_progress_tool_call(progress_step: &ProgressStep) -> bool {
    progress_step
        .tool_call_status_by_task_id
        .values()
        .any(|status| *status == ToolCallStatus::InProgress)
}

pub(super) fn resolve_progress_step_status(progress_step: &ProgressStep) -> ProgressStepStatus {
    if progress_step.tool_call_status_by_task_id.is_empty() {
        return ProgressStepStatus::InProgress;
    }

    if progress_step_has_in_progress_tool_call(progress_step) {
        return ProgressStepStatus::InProgress;
    }

    ProgressStepStatus::Complete
}

fn build_task_ownership_key(owner_id: &str, task_id: &str) -> String {
    format!("{owner_id}:{task_id}")
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressStepStatus, ProgressStreamState, ResolveToolStartedProgressStepOutput,
        resolve_progress_step_status,
    };

    #[test]
    fn reopens_latest_completed_progress_step_for_new_tool_activity() {
        let mut state = ProgressStreamState::new();
        let progress_step_id = state.create_progress_step("owner-1", "Collect evidence");
        state.set_active_progress_step("owner-1", progress_step_id.clone());
        state
            .mark_progress_step_completed(&progress_step_id)
            .expect("complete existing progress step");
        state.clear_active_progress_step("owner-1");

        let resolved =
            state.resolve_progress_step_for_tool_started("owner-1", "task-1", "Tool executions");

        assert_eq!(
            resolved,
            ResolveToolStartedProgressStepOutput {
                progress_step_id: "progress-step-2".to_string(),
                reopened_from_progress_step_id: Some(progress_step_id),
            }
        );

        let reopened_progress_step = state
            .progress_step(&resolved.progress_step_id)
            .expect("reopened progress step");
        assert_eq!(reopened_progress_step.title, "Collect evidence");
    }

    #[test]
    fn completes_active_progress_step_only_after_all_tools_complete() {
        let mut state = ProgressStreamState::new();
        let progress_step_id = state.create_progress_step("owner-1", "Collect evidence");
        state.set_active_progress_step("owner-1", progress_step_id.clone());

        state.upsert_progress_step_tool_call_status(
            &progress_step_id,
            "owner-1",
            "task-1",
            ProgressStepStatus::InProgress,
        );
        assert!(
            state
                .complete_active_progress_step_if_idle("owner-1")
                .is_none()
        );

        state.upsert_progress_step_tool_call_status(
            &progress_step_id,
            "owner-1",
            "task-1",
            ProgressStepStatus::Complete,
        );
        let completed_progress_step = state
            .complete_active_progress_step_if_idle("owner-1")
            .expect("complete idle progress step");
        assert_eq!(
            resolve_progress_step_status(&completed_progress_step),
            ProgressStepStatus::Complete
        );
    }
}
