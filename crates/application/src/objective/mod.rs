//! Objective-driven autonomous work: scheduling, the durable Observe → Work → Decide → Report →
//! Learn cycle, and human collaboration.

mod brief;
mod clock;
mod cycle;
mod decide;
mod policy;
mod reconcile;
mod report;
mod scheduler;
mod store;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
pub(crate) mod tests_support;

pub use clock::{Clock, SystemClock};
pub use cycle::{ObjectiveCycleDeps, ObjectiveCycleRunner};
pub use scheduler::{ObjectiveScheduler, ObjectiveSchedulerDeps};
pub use store::WorkStateStore;
