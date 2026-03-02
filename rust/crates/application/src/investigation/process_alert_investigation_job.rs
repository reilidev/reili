use sre_shared::types::AlertInvestigationJob;

use super::execute_investigation_job::{
    ExecuteInvestigationJobInput, InvestigationExecutionDeps, execute_investigation_job,
};
use super::execution_errors::ExecuteInvestigationJobError;

pub type ProcessAlertInvestigationJobUseCaseDeps = InvestigationExecutionDeps;

pub struct ProcessAlertInvestigationJobUseCase {
    deps: ProcessAlertInvestigationJobUseCaseDeps,
}

impl ProcessAlertInvestigationJobUseCase {
    #[must_use]
    pub fn new(deps: ProcessAlertInvestigationJobUseCaseDeps) -> Self {
        Self { deps }
    }

    pub async fn handle(
        &self,
        job: AlertInvestigationJob,
    ) -> Result<(), ExecuteInvestigationJobError> {
        execute_investigation_job(ExecuteInvestigationJobInput {
            job_type: job.job_type,
            job_id: job.job_id,
            retry_count: job.retry_count,
            payload: job.payload,
            deps: clone_deps(&self.deps),
        })
        .await
    }
}

fn clone_deps(deps: &InvestigationExecutionDeps) -> InvestigationExecutionDeps {
    InvestigationExecutionDeps {
        slack_reply_port: deps.slack_reply_port.clone(),
        slack_progress_stream_port: deps.slack_progress_stream_port.clone(),
        slack_thread_history_port: deps.slack_thread_history_port.clone(),
        investigation_resources: super::execute_investigation_job::clone_investigation_resources(
            &deps.investigation_resources,
        ),
        coordinator_runner: deps.coordinator_runner.clone(),
        synthesizer_runner: deps.synthesizer_runner.clone(),
        logger: deps.logger.clone(),
    }
}
