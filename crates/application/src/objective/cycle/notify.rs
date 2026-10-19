use reili_core::messaging::slack::SlackPostMessageInput;
use reili_core::objective::ObjectiveDefinition;

use super::super::report::{Activity, compose_report};
use super::ObjectiveCycleRunner;

impl ObjectiveCycleRunner {
    /// Returns whether a report was posted. Slack failures are logged, never fatal: the work is
    /// already on the tracker.
    pub(super) async fn notify(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        message: Option<&str>,
        activity: &Activity,
    ) -> bool {
        let Some(text) = compose_report(message, activity) else {
            return false;
        };
        let result = self
            .deps
            .report_port
            .post_message(SlackPostMessageInput {
                channel: objective.report_channel_id.clone(),
                thread_ts: None,
                text,
            })
            .await;
        match result {
            Ok(()) => true,
            Err(error) => {
                self.log(
                    "objective_cycle_report_failed",
                    objective,
                    run_id,
                    Some(&error.message),
                );
                false
            }
        }
    }
}
