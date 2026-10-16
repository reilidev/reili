//! Goal-driven autonomous work: scheduling, the durable Observe → Investigate → Decide → Report →
//! Learn cycle, and human collaboration.

mod brief;
mod clock;
mod cycle;
mod decide;
mod policy;
mod reconcile;
mod report;
mod scheduler;
mod slack;
mod store;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
pub(crate) mod tests_support;

pub use clock::SystemClock;
pub use cycle::{GoalCycleDeps, GoalCycleRunner};
pub use scheduler::{GoalScheduler, GoalSchedulerDeps};
pub use slack::{
    SlackGoalAlertTriggerHandler, SlackGoalCommandHandler, SlackGoalCommandHandlerDeps,
};
pub use store::WorkStateStore;
