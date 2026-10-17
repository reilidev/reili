//! Shared fixtures for tests in this crate and downstream crates (`test-support` feature).

use super::item::{NewWorkItem, WorkItemKey};
use crate::objective::{
    ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectivePermissions, ObjectiveSchedule,
    ObjectiveScope,
};

#[must_use]
pub fn sample_objective(id: &str) -> ObjectiveDefinition {
    ObjectiveDefinition {
        id: ObjectiveId::new(id).unwrap_or_else(|_| unreachable!()),
        description: "Continuously improve checkout service reliability.".to_string(),
        scope: ObjectiveScope {
            services: vec!["checkout-api".to_string()],
            repositories: vec!["acme/checkout".to_string()],
            environments: vec!["production".to_string()],
        },
        schedule: ObjectiveSchedule::parse("0 */6 * * *", "Asia/Tokyo")
            .unwrap_or_else(|_| unreachable!()),
        report_channel_id: "C0123456789".to_string(),
        budget: ObjectiveBudget::default(),
        permissions: ObjectivePermissions::default(),
        enabled: true,
    }
}

#[must_use]
pub fn sample_new_work_item(objective: &str, key: &str) -> NewWorkItem {
    NewWorkItem {
        objective_id: ObjectiveId::new(objective).unwrap_or_else(|_| unreachable!()),
        key: WorkItemKey::normalize(key).unwrap_or_else(|| unreachable!()),
        title: format!("Sample {key}"),
        body: "sample problem".to_string(),
        needs_decision: false,
    }
}
