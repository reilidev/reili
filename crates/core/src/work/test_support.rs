use super::item::NewWorkItem;
use crate::objective::{ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectiveSchedule};

#[must_use]
pub fn sample_objective(id: &str) -> ObjectiveDefinition {
    ObjectiveDefinition {
        id: ObjectiveId::new(id).unwrap_or_else(|_| unreachable!()),
        description: "Continuously improve checkout service reliability.".to_string(),
        schedule: ObjectiveSchedule::parse("0 */6 * * *", "Asia/Tokyo")
            .unwrap_or_else(|_| unreachable!()),
        report_channel_id: "C0123456789".to_string(),
        budget: ObjectiveBudget::default(),
        enabled: true,
        takes_unlinked_work: false,
    }
}

#[must_use]
pub fn sample_new_work_item(objective: &str, title: &str) -> NewWorkItem {
    NewWorkItem {
        objective_id: ObjectiveId::new(objective).unwrap_or_else(|_| unreachable!()),
        title: title.to_string(),
        body: "sample problem".to_string(),
        needs_decision: false,
    }
}
