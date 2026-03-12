use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReasoningScopeStatus {
    InProgress,
    Complete,
}

pub(super) type ReasoningScopeToolStatus = ReasoningScopeStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReasoningScope {
    pub scope_id: String,
    pub owner_id: String,
    pub title: String,
    pub tool_status_by_task_id: HashMap<String, ReasoningScopeToolStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolveToolStartedScopeOutput {
    pub scope_id: String,
    pub reopened_from_scope_id: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct ProgressStreamState {
    active_reasoning_scope_id_by_owner_id: HashMap<String, String>,
    reasoning_scope_by_id: HashMap<String, ReasoningScope>,
    reasoning_scope_id_by_task_key: HashMap<String, String>,
    completed_reasoning_scope_ids_by_owner_id: HashMap<String, HashSet<String>>,
    latest_completed_reasoning_scope_id_by_owner_id: HashMap<String, String>,
    next_scope_number: u64,
}

impl ProgressStreamState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn create_reasoning_scope(&mut self, owner_id: &str, title: &str) -> String {
        let scope_id = self.create_reasoning_scope_id();
        let scope = ReasoningScope {
            scope_id: scope_id.clone(),
            owner_id: owner_id.to_string(),
            title: title.to_string(),
            tool_status_by_task_id: HashMap::new(),
        };
        self.reasoning_scope_by_id.insert(scope_id.clone(), scope);
        scope_id
    }

    pub(super) fn set_active_scope(&mut self, owner_id: &str, scope_id: String) {
        self.active_reasoning_scope_id_by_owner_id
            .insert(owner_id.to_string(), scope_id);
    }

    pub(super) fn clear_active_scope(&mut self, owner_id: &str) {
        self.active_reasoning_scope_id_by_owner_id.remove(owner_id);
    }

    pub(super) fn resolve_scope_for_tool_started(
        &mut self,
        owner_id: &str,
        task_id: &str,
        reopened_scope_default_title: &str,
    ) -> ResolveToolStartedScopeOutput {
        let task_ownership_key = build_task_ownership_key(owner_id, task_id);
        if let Some(existing_scope_id) = self
            .reasoning_scope_id_by_task_key
            .get(&task_ownership_key)
            .filter(|scope_id| self.reasoning_scope_by_id.contains_key(*scope_id))
        {
            return ResolveToolStartedScopeOutput {
                scope_id: existing_scope_id.clone(),
                reopened_from_scope_id: None,
            };
        }

        if let Some(active_scope_id) = self
            .active_reasoning_scope_id_by_owner_id
            .get(owner_id)
            .cloned()
        {
            if self.reasoning_scope_by_id.contains_key(&active_scope_id) {
                self.reasoning_scope_id_by_task_key
                    .insert(task_ownership_key, active_scope_id.clone());
                return ResolveToolStartedScopeOutput {
                    scope_id: active_scope_id,
                    reopened_from_scope_id: None,
                };
            }

            self.active_reasoning_scope_id_by_owner_id.remove(owner_id);
        }

        let last_completed_scope = self.resolve_latest_completed_scope_by_owner_id(owner_id);
        let reopened_scope_title = last_completed_scope.as_ref().map_or_else(
            || reopened_scope_default_title.to_string(),
            |scope| scope.title.clone(),
        );
        let reopened_scope_id = self.create_reasoning_scope(owner_id, &reopened_scope_title);
        self.set_active_scope(owner_id, reopened_scope_id.clone());

        ResolveToolStartedScopeOutput {
            scope_id: reopened_scope_id,
            reopened_from_scope_id: last_completed_scope.map(|scope| scope.scope_id),
        }
    }

    pub(super) fn resolve_scope_for_tool_completed(
        &self,
        owner_id: &str,
        task_id: &str,
    ) -> Option<String> {
        let task_ownership_key = build_task_ownership_key(owner_id, task_id);
        self.reasoning_scope_id_by_task_key
            .get(&task_ownership_key)
            .filter(|scope_id| self.reasoning_scope_by_id.contains_key(*scope_id))
            .cloned()
    }

    pub(super) fn upsert_scope_tool_status(
        &mut self,
        scope_id: &str,
        owner_id: &str,
        task_id: &str,
        status: ReasoningScopeToolStatus,
    ) {
        if let Some(scope) = self.reasoning_scope_by_id.get_mut(scope_id) {
            scope
                .tool_status_by_task_id
                .insert(task_id.to_string(), status);
        }

        self.reasoning_scope_id_by_task_key.insert(
            build_task_ownership_key(owner_id, task_id),
            scope_id.to_string(),
        );
    }

    pub(super) fn mark_scope_incomplete(&mut self, owner_id: &str, scope_id: &str) {
        self.resolve_completed_reasoning_scope_ids_by_owner_id(owner_id)
            .remove(scope_id);
    }

    pub(super) fn complete_active_scope_if_idle(
        &mut self,
        owner_id: &str,
    ) -> Option<ReasoningScope> {
        let active_scope_id = self
            .active_reasoning_scope_id_by_owner_id
            .get(owner_id)
            .cloned()?;
        let active_scope = match self.reasoning_scope_by_id.get(&active_scope_id).cloned() {
            Some(scope) => scope,
            None => {
                self.active_reasoning_scope_id_by_owner_id.remove(owner_id);
                return None;
            }
        };

        if scope_has_in_progress_tool(&active_scope) {
            return None;
        }

        self.mark_scope_completed(&active_scope_id)
    }

    pub(super) fn mark_scope_completed(&mut self, scope_id: &str) -> Option<ReasoningScope> {
        let scope = self.reasoning_scope_by_id.get(scope_id).cloned()?;
        let completed_scope_ids =
            self.resolve_completed_reasoning_scope_ids_by_owner_id(&scope.owner_id);
        if completed_scope_ids.contains(scope_id) {
            return None;
        }
        completed_scope_ids.insert(scope_id.to_string());
        self.latest_completed_reasoning_scope_id_by_owner_id
            .insert(scope.owner_id.clone(), scope_id.to_string());
        Some(scope)
    }

    pub(super) fn scope(&self, scope_id: &str) -> Option<ReasoningScope> {
        self.reasoning_scope_by_id.get(scope_id).cloned()
    }

    pub(super) fn scope_ids_for_owner(&self, owner_id: &str) -> Vec<String> {
        self.reasoning_scope_by_id
            .values()
            .filter(|scope| scope.owner_id == owner_id)
            .map(|scope| scope.scope_id.clone())
            .collect()
    }

    fn create_reasoning_scope_id(&mut self) -> String {
        self.next_scope_number = self.next_scope_number.saturating_add(1);
        let mut scope_id = format!("reasoning-{}", self.next_scope_number);
        while self.reasoning_scope_by_id.contains_key(&scope_id) {
            self.next_scope_number = self.next_scope_number.saturating_add(1);
            scope_id = format!("reasoning-{}", self.next_scope_number);
        }

        scope_id
    }

    fn resolve_latest_completed_scope_by_owner_id(
        &mut self,
        owner_id: &str,
    ) -> Option<ReasoningScope> {
        let latest_completed_scope_id = self
            .latest_completed_reasoning_scope_id_by_owner_id
            .get(owner_id)
            .cloned()?;
        let scope = self
            .reasoning_scope_by_id
            .get(&latest_completed_scope_id)
            .cloned();
        if scope.is_none() {
            self.latest_completed_reasoning_scope_id_by_owner_id
                .remove(owner_id);
        }

        scope
    }

    fn resolve_completed_reasoning_scope_ids_by_owner_id(
        &mut self,
        owner_id: &str,
    ) -> &mut HashSet<String> {
        self.completed_reasoning_scope_ids_by_owner_id
            .entry(owner_id.to_string())
            .or_default()
    }
}

pub(super) fn scope_has_in_progress_tool(scope: &ReasoningScope) -> bool {
    scope
        .tool_status_by_task_id
        .values()
        .any(|status| *status == ReasoningScopeToolStatus::InProgress)
}

pub(super) fn resolve_reasoning_scope_status(scope: &ReasoningScope) -> ReasoningScopeStatus {
    if scope.tool_status_by_task_id.is_empty() {
        return ReasoningScopeStatus::InProgress;
    }

    if scope_has_in_progress_tool(scope) {
        return ReasoningScopeStatus::InProgress;
    }

    ReasoningScopeStatus::Complete
}

fn build_task_ownership_key(owner_id: &str, task_id: &str) -> String {
    format!("{owner_id}:{task_id}")
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressStreamState, ReasoningScopeStatus, ResolveToolStartedScopeOutput,
        resolve_reasoning_scope_status,
    };

    #[test]
    fn reopens_latest_completed_scope_for_new_tool_activity() {
        let mut state = ProgressStreamState::new();
        let scope_id = state.create_reasoning_scope("owner-1", "Collect evidence");
        state.set_active_scope("owner-1", scope_id.clone());
        state
            .mark_scope_completed(&scope_id)
            .expect("complete existing scope");
        state.clear_active_scope("owner-1");

        let resolved = state.resolve_scope_for_tool_started("owner-1", "task-1", "Tool executions");

        assert_eq!(
            resolved,
            ResolveToolStartedScopeOutput {
                scope_id: "reasoning-2".to_string(),
                reopened_from_scope_id: Some(scope_id),
            }
        );

        let reopened_scope = state.scope(&resolved.scope_id).expect("reopened scope");
        assert_eq!(reopened_scope.title, "Collect evidence");
    }

    #[test]
    fn completes_active_scope_only_after_all_tools_complete() {
        let mut state = ProgressStreamState::new();
        let scope_id = state.create_reasoning_scope("owner-1", "Collect evidence");
        state.set_active_scope("owner-1", scope_id.clone());

        state.upsert_scope_tool_status(
            &scope_id,
            "owner-1",
            "task-1",
            ReasoningScopeStatus::InProgress,
        );
        assert!(state.complete_active_scope_if_idle("owner-1").is_none());

        state.upsert_scope_tool_status(
            &scope_id,
            "owner-1",
            "task-1",
            ReasoningScopeStatus::Complete,
        );
        let completed_scope = state
            .complete_active_scope_if_idle("owner-1")
            .expect("complete idle scope");
        assert_eq!(
            resolve_reasoning_scope_status(&completed_scope),
            ReasoningScopeStatus::Complete
        );
    }
}
