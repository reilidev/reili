//! Objective-driven autonomous work on the team's tracker: scheduling and the Observe → Work →
//! Decide → Report → Learn cycle.

mod brief;
mod clock;
mod cycle;
mod decide;
mod policy;
mod render;
mod report;
mod scheduler;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
pub(crate) mod tests_support;

pub use clock::{Clock, SystemClock};
pub use cycle::{CycleOutcome, ObjectiveCycleDeps, ObjectiveCycleRunner};
pub use scheduler::{ObjectiveScheduler, ObjectiveSchedulerDeps};
