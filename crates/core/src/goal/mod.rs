//! Long-term goals: configuration-owned definitions Reili pursues autonomously.

pub mod definition;
pub mod fingerprint;
pub mod schedule;

pub use definition::{
    GoalBudget, GoalDefinition, GoalDefinitionError, GoalId, GoalPermissions, GoalReportPolicy,
    GoalScope,
};
pub use fingerprint::stable_fingerprint;
pub use schedule::{GoalSchedule, ScheduleError, truncate_to_minute};

#[cfg(test)]
pub(crate) use definition::test_support;
