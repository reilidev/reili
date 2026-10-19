use std::sync::Arc;

use reili_adapters::outbound::github::{GitHubWorkStateConfig, GitHubWorkStateRepository};
use reili_application::TaskLogger;
use reili_application::objective::{
    Clock, ObjectiveCycleDeps, ObjectiveCycleRunner, ObjectiveScheduler, ObjectiveSchedulerDeps,
    SystemClock,
};
use reili_core::cycle_agent::CycleAgentPort;
use reili_core::messaging::slack::SlackPostMessagePort;
use reili_core::task::TaskResources;
use reili_core::work::WorkStatePort;

use super::RuntimeBootstrapError;
use crate::config::{AppConfig, WorkStateConfig};

pub(crate) struct BuildObjectiveSchedulerInput<'a> {
    pub(crate) config: &'a AppConfig,
    pub(crate) cycle_agent: Arc<dyn CycleAgentPort>,
    pub(crate) task_resources: TaskResources,
    pub(crate) report_port: Arc<dyn SlackPostMessagePort>,
    pub(crate) logger: Arc<dyn TaskLogger>,
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
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let cycle_runner = Arc::new(ObjectiveCycleRunner::new(ObjectiveCycleDeps {
        work_state,
        cycle_agent: input.cycle_agent,
        report_port: input.report_port,
        task_resources: input.task_resources,
        clock: Arc::clone(&clock),
        logger: Arc::clone(&input.logger),
    }));
    Ok(Some(ObjectiveScheduler::new(ObjectiveSchedulerDeps {
        objectives: input.config.objectives.clone(),
        cycle_runner,
        clock,
        logger: input.logger,
    })))
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
