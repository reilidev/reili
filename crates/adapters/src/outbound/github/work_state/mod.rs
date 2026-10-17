//! Work State persistence on GitHub Issues.

mod codec;
mod repository;
#[cfg(test)]
mod tests;

pub use repository::{GitHubWorkStateConfig, GitHubWorkStateRepository};

#[cfg(test)]
pub(crate) mod tests_support {
    use reili_core::objective::{
        ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectivePermissions,
        ObjectiveReportPolicy, ObjectiveSchedule, ObjectiveScope,
    };

    pub(crate) fn objective() -> ObjectiveDefinition {
        ObjectiveDefinition {
            id: ObjectiveId::new("checkout").expect("id"),
            description: "Improve checkout reliability".to_string(),
            scope: ObjectiveScope::default(),
            schedule: ObjectiveSchedule::parse("0 * * * *", "UTC").expect("schedule"),
            report: ObjectiveReportPolicy {
                channel_id: "C1".to_string(),
                min_interval_minutes: 60,
                max_reports_per_day: 4,
            },
            budget: ObjectiveBudget::default(),
            permissions: ObjectivePermissions::default(),
            enabled: true,
        }
    }
}
