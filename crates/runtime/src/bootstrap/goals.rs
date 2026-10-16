//! Wiring for goal-driven autonomy: GitHub Work State, the cycle runner, and the scheduler.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Duration;
use reili_adapters::outbound::github::{GitHubWorkStateConfig, GitHubWorkStateRepository};
use reili_adapters::outbound::slack::{SlackGoalReportAdapter, SlackWebApiClient};
use reili_application::TaskLogger;
use reili_application::goal::{
    Clock, GoalCycleDeps, GoalCycleRunner, GoalScheduler, GoalSchedulerDeps, SystemClock,
    WorkStateStore,
};
use reili_core::investigation::InvestigationRunnerPort;
use reili_core::messaging::slack::GoalReportPort;
use reili_core::task::TaskResources;
use reili_core::work::WorkStateRepositoryPort;

use super::RuntimeBootstrapError;
use crate::config::{AppConfig, AutonomyConfig};
use reili_core::goal::GoalDefinition;

pub(crate) struct BuildGoalSchedulerInput<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) investigation_runner: Arc<dyn InvestigationRunnerPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn GoalReportPort>,
    pub(crate) logger: Arc<dyn TaskLogger>,
}

pub(crate) fn build_goal_report_port(client: Arc<SlackWebApiClient>) -> Arc<dyn GoalReportPort> {
    Arc::new(SlackGoalReportAdapter::new(client))
}

/// Returns `None` when no goals are configured (the scheduler is not started at all).
pub(crate) fn build_goal_scheduler(
    input: BuildGoalSchedulerInput<'_>,
) -> Result<Option<Arc<GoalScheduler>>, RuntimeBootstrapError> {
    if input.config.goals.is_empty() {
        return Ok(None);
    }
    let Some(work_state) = input.config.work_state.as_ref() else {
        return Ok(None);
    };
    let repository: Arc<dyn WorkStateRepositoryPort> =
        Arc::new(GitHubWorkStateRepository::new(&GitHubWorkStateConfig {
            repository: work_state.repository.clone(),
            app_id: work_state.app_id.clone(),
            private_key: work_state.private_key.clone(),
            installation_id: work_state.installation_id,
            api_base_url: work_state.api_base_url.clone(),
            web_base_url: work_state.web_base_url.clone(),
        })?);
    Ok(Some(assemble_goal_scheduler(AssembleGoalSchedulerInput {
        goals: input.config.goals.clone(),
        autonomy: &input.config.autonomy,
        repository,
        investigation_runner: input.investigation_runner,
        task_resources: input.task_resources,
        report_port: input.report_port,
        clock: Arc::new(SystemClock),
        logger: input.logger,
        instance_id: instance_id(),
    })))
}

pub(crate) struct AssembleGoalSchedulerInput<'a> {
    pub(crate) goals: Vec<GoalDefinition>,
    pub(crate) autonomy: &'a AutonomyConfig,
    pub(crate) repository: Arc<dyn WorkStateRepositoryPort>,
    pub(crate) investigation_runner: Arc<dyn InvestigationRunnerPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn GoalReportPort>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) logger: Arc<dyn TaskLogger>,
    pub(crate) instance_id: String,
}

pub(crate) fn assemble_goal_scheduler(input: AssembleGoalSchedulerInput<'_>) -> Arc<GoalScheduler> {
    let store = WorkStateStore::new(input.repository);
    let clock = input.clock;
    let autonomy = input.autonomy;
    let cycle_runner = Arc::new(GoalCycleRunner::new(GoalCycleDeps {
        store: store.clone(),
        investigation_runner: input.investigation_runner,
        report_port: input.report_port,
        task_resources: input.task_resources,
        clock: Arc::clone(&clock),
        logger: Arc::clone(&input.logger),
        instance_id: input.instance_id,
        lease_duration: Duration::minutes(i64::from(autonomy.lease_minutes)),
    }));
    GoalScheduler::new(GoalSchedulerDeps {
        goals: input.goals,
        cycle_runner,
        store,
        clock,
        logger: input.logger,
        tick_interval: std::time::Duration::from_secs(autonomy.tick_interval_seconds),
        max_concurrent_cycles: usize::try_from(autonomy.max_concurrent_cycles).unwrap_or(1),
    })
}

#[cfg(test)]
#[path = "goals_integration_tests.rs"]
mod integration_tests;

fn instance_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format!("reili-{}-{millis}", std::process::id())
}
