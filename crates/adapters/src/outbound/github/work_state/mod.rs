//! Work State persistence on GitHub Issues.

mod codec;
mod repository;
#[cfg(test)]
mod tests;

pub use repository::{GitHubWorkStateConfig, GitHubWorkStateRepository};

#[cfg(test)]
pub(crate) mod tests_support {
    use reili_core::goal::{
        GoalBudget, GoalDefinition, GoalId, GoalPermissions, GoalReportPolicy, GoalSchedule,
        GoalScope,
    };

    pub(crate) fn goal() -> GoalDefinition {
        GoalDefinition {
            id: GoalId::new("checkout").expect("id"),
            objective: "Improve checkout reliability".to_string(),
            scope: GoalScope::default(),
            schedule: GoalSchedule::parse("0 * * * *", "UTC").expect("schedule"),
            report: GoalReportPolicy {
                channel_id: "C1".to_string(),
                min_interval_minutes: 60,
                max_reports_per_day: 4,
            },
            budget: GoalBudget::default(),
            permissions: GoalPermissions::default(),
            enabled: true,
        }
    }
}
