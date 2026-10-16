use reili_core::goal::{
    GoalBudget, GoalDefinition, GoalId, GoalPermissions, GoalReportPolicy, GoalSchedule, GoalScope,
};

pub(crate) fn sample_goal(id: &str) -> GoalDefinition {
    GoalDefinition {
        id: GoalId::new(id).expect("goal id"),
        objective: "Continuously improve checkout service reliability.".to_string(),
        scope: GoalScope {
            services: vec!["checkout-api".to_string()],
            repositories: vec!["acme/checkout".to_string()],
            environments: vec!["production".to_string()],
        },
        schedule: GoalSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("schedule"),
        report: GoalReportPolicy {
            channel_id: "C0123456789".to_string(),
            min_interval_minutes: 60,
            max_reports_per_day: 6,
        },
        budget: GoalBudget::default(),
        permissions: GoalPermissions::default(),
        enabled: true,
    }
}
