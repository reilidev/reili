//! Report: post the cycle's noteworthy changes to Slack once per scheduled slot.

use chrono::{DateTime, Utc};
use reili_core::messaging::slack::ObjectiveReportMessage;
use reili_core::objective::ObjectiveDefinition;

use super::super::report::{Activity, compose_report, report_key};
use super::ObjectiveCycleRunner;

impl ObjectiveCycleRunner {
    /// Returns whether a report was posted. Slack failures are logged, never fatal: the work is
    /// already on the tracker.
    pub(super) async fn notify(
        &self,
        objective: &ObjectiveDefinition,
        run_id: &str,
        slot: DateTime<Utc>,
        summary: &str,
        activity: &Activity,
    ) -> bool {
        if !objective.permissions.post_reports {
            return false;
        }
        let Some(text) = compose_report(objective, summary, run_id, activity) else {
            return false;
        };
        let key = report_key(objective, slot);
        let port = &self.deps.report_port;
        let result = match port
            .find_posted_report(&objective.report_channel_id, &key)
            .await
        {
            Ok(Some(_)) => return false,
            Ok(None) => port
                .post_report(ObjectiveReportMessage {
                    channel_id: objective.report_channel_id.clone(),
                    objective_id: objective.id.to_string(),
                    report_key: key,
                    text,
                })
                .await
                .map(|_| ()),
            Err(error) => Err(error),
        };
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
