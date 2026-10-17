//! Wiring for objective-driven autonomy: GitHub Work State, the cycle runner, and the scheduler.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Duration;
use reili_adapters::outbound::github::{GitHubWorkStateConfig, GitHubWorkStateRepository};
use reili_adapters::outbound::slack::{SlackObjectiveReportAdapter, SlackWebApiClient};
use reili_application::TaskLogger;
use reili_application::objective::{
    Clock, ObjectiveCycleDeps, ObjectiveCycleRunner, ObjectiveScheduler, ObjectiveSchedulerDeps,
    SystemClock, WorkStateStore,
};
use reili_core::investigation::InvestigationRunnerPort;
use reili_core::messaging::slack::ObjectiveReportPort;
use reili_core::task::TaskResources;
use reili_core::work::WorkStateRepositoryPort;

use super::RuntimeBootstrapError;
use crate::config::{AppConfig, AutonomyConfig};
use reili_core::objective::ObjectiveDefinition;

pub(crate) struct BuildObjectiveSchedulerInput<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) investigation_runner: Arc<dyn InvestigationRunnerPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn ObjectiveReportPort>,
    pub(crate) logger: Arc<dyn TaskLogger>,
}

pub(crate) fn build_objective_report_port(
    client: Arc<SlackWebApiClient>,
) -> Arc<dyn ObjectiveReportPort> {
    Arc::new(SlackObjectiveReportAdapter::new(client))
}

/// Returns `None` when no objectives are configured (the scheduler is not started at all).
pub(crate) fn build_objective_scheduler(
    input: BuildObjectiveSchedulerInput<'_>,
) -> Result<Option<Arc<ObjectiveScheduler>>, RuntimeBootstrapError> {
    if input.config.objectives.is_empty() {
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
    Ok(Some(assemble_objective_scheduler(
        AssembleObjectiveSchedulerInput {
            objectives: input.config.objectives.clone(),
            autonomy: &input.config.autonomy,
            repository,
            investigation_runner: input.investigation_runner,
            task_resources: input.task_resources,
            report_port: input.report_port,
            clock: Arc::new(SystemClock),
            logger: input.logger,
            instance_id: instance_id(),
        },
    )))
}

pub(crate) struct AssembleObjectiveSchedulerInput<'a> {
    pub(crate) objectives: Vec<ObjectiveDefinition>,
    pub(crate) autonomy: &'a AutonomyConfig,
    pub(crate) repository: Arc<dyn WorkStateRepositoryPort>,
    pub(crate) investigation_runner: Arc<dyn InvestigationRunnerPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn ObjectiveReportPort>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) logger: Arc<dyn TaskLogger>,
    pub(crate) instance_id: String,
}

pub(crate) fn assemble_objective_scheduler(
    input: AssembleObjectiveSchedulerInput<'_>,
) -> Arc<ObjectiveScheduler> {
    let store = WorkStateStore::new(input.repository);
    let clock = input.clock;
    let autonomy = input.autonomy;
    let cycle_runner = Arc::new(ObjectiveCycleRunner::new(ObjectiveCycleDeps {
        store: store.clone(),
        investigation_runner: input.investigation_runner,
        report_port: input.report_port,
        task_resources: input.task_resources,
        clock: Arc::clone(&clock),
        logger: Arc::clone(&input.logger),
        instance_id: input.instance_id,
        lease_duration: Duration::minutes(i64::from(autonomy.lease_minutes)),
    }));
    ObjectiveScheduler::new(ObjectiveSchedulerDeps {
        objectives: input.objectives,
        cycle_runner,
        store,
        clock,
        logger: input.logger,
        tick_interval: std::time::Duration::from_secs(autonomy.tick_interval_seconds),
        max_concurrent_cycles: usize::try_from(autonomy.max_concurrent_cycles).unwrap_or(1),
    })
}

#[cfg(test)]
#[path = "objectives_integration_tests.rs"]
mod integration_tests;

fn instance_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    format!("reili-{}-{millis}", std::process::id())
}
