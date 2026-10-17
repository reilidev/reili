//! Long-term objectives: configuration-owned definitions Reili pursues autonomously.

pub mod definition;
pub mod fingerprint;
pub mod schedule;

pub use definition::{
    ObjectiveBudget, ObjectiveDefinition, ObjectiveDefinitionError, ObjectiveId,
    ObjectivePermissions, ObjectiveReportPolicy, ObjectiveScope,
};
pub use fingerprint::stable_fingerprint;
pub use schedule::{ObjectiveSchedule, ScheduleError, truncate_to_minute};
