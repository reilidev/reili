use reili_core::objective::{
    ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectivePermissions, ObjectiveReportPolicy,
    ObjectiveSchedule, ObjectiveScope,
};

pub(crate) fn sample_objective(id: &str) -> ObjectiveDefinition {
    ObjectiveDefinition {
        id: ObjectiveId::new(id).expect("objective id"),
        description: "Continuously improve checkout service reliability.".to_string(),
        scope: ObjectiveScope {
            services: vec!["checkout-api".to_string()],
            repositories: vec!["acme/checkout".to_string()],
            environments: vec!["production".to_string()],
        },
        schedule: ObjectiveSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("schedule"),
        report: ObjectiveReportPolicy {
            channel_id: "C0123456789".to_string(),
            min_interval_minutes: 60,
            max_reports_per_day: 6,
        },
        budget: ObjectiveBudget::default(),
        permissions: ObjectivePermissions::default(),
        enabled: true,
    }
}
