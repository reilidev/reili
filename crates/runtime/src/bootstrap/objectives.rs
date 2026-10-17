//! Wiring for objective-driven autonomy: the work tracker backend, the cycle runner, and the
//! scheduler.

use std::sync::Arc;

use reili_adapters::outbound::github::{GitHubWorkStateConfig, GitHubWorkStateRepository};
use reili_adapters::outbound::slack::{SlackObjectiveReportAdapter, SlackWebApiClient};
use reili_application::TaskLogger;
use reili_application::objective::{
    Clock, ObjectiveCycleDeps, ObjectiveCycleRunner, ObjectiveScheduler, ObjectiveSchedulerDeps,
    SystemClock,
};
use reili_core::cycle_agent::CycleAgentPort;
use reili_core::messaging::slack::ObjectiveReportPort;
use reili_core::objective::ObjectiveDefinition;
use reili_core::task::TaskResources;
use reili_core::work::WorkStatePort;

use super::RuntimeBootstrapError;
use crate::config::{AppConfig, AutonomyConfig, WorkStateConfig};

pub(crate) struct BuildObjectiveSchedulerInput<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) cycle_agent: Arc<dyn CycleAgentPort>,
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
    let work_state = build_work_state(work_state)?;
    Ok(Some(assemble_objective_scheduler(
        AssembleObjectiveSchedulerInput {
            objectives: input.config.objectives.clone(),
            autonomy: &input.config.autonomy,
            work_state,
            cycle_agent: input.cycle_agent,
            task_resources: input.task_resources,
            report_port: input.report_port,
            clock: Arc::new(SystemClock),
            logger: input.logger,
        },
    )))
}

fn build_work_state(
    config: &WorkStateConfig,
) -> Result<Arc<dyn WorkStatePort>, RuntimeBootstrapError> {
    match config {
        WorkStateConfig::GitHub(github) => Ok(Arc::new(GitHubWorkStateRepository::new(
            &GitHubWorkStateConfig {
                repository: github.repository.clone(),
                app_id: github.app_id.clone(),
                private_key: github.private_key.clone(),
                installation_id: github.installation_id,
                api_base_url: github.api_base_url.clone(),
                web_base_url: github.web_base_url.clone(),
            },
        )?)),
    }
}

pub(crate) struct AssembleObjectiveSchedulerInput<'a> {
    pub(crate) objectives: Vec<ObjectiveDefinition>,
    pub(crate) autonomy: &'a AutonomyConfig,
    pub(crate) work_state: Arc<dyn WorkStatePort>,
    pub(crate) cycle_agent: Arc<dyn CycleAgentPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn ObjectiveReportPort>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) logger: Arc<dyn TaskLogger>,
}

pub(crate) fn assemble_objective_scheduler(
    input: AssembleObjectiveSchedulerInput<'_>,
) -> Arc<ObjectiveScheduler> {
    let clock = input.clock;
    let autonomy = input.autonomy;
    let cycle_runner = Arc::new(ObjectiveCycleRunner::new(ObjectiveCycleDeps {
        work_state: input.work_state,
        cycle_agent: input.cycle_agent,
        report_port: input.report_port,
        task_resources: input.task_resources,
        clock: Arc::clone(&clock),
        logger: Arc::clone(&input.logger),
    }));
    ObjectiveScheduler::new(ObjectiveSchedulerDeps {
        objectives: input.objectives,
        cycle_runner,
        clock,
        logger: input.logger,
        tick_interval: std::time::Duration::from_secs(autonomy.tick_interval_seconds),
        max_concurrent_cycles: usize::try_from(autonomy.max_concurrent_cycles).unwrap_or(1),
    })
}

#[cfg(test)]
#[path = "objectives_integration_tests.rs"]
mod integration_tests;
