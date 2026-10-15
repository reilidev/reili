//! Goal-driven autonomous work: scheduling, the durable Observe → Investigate → Verify → Decide →
//! Report → Learn cycle, human collaboration, and outcome evaluation.

pub mod brief;
pub mod clock;
pub mod cycle;
pub mod decide;
pub mod evaluation;
pub mod gate;
pub mod outcome;
pub mod reconcile;
pub mod report;
pub mod scheduler;
pub mod slack;
pub mod store;

#[cfg(test)]
mod scenarios;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
pub(crate) mod tests_support;

pub use clock::{Clock, SystemClock};
pub use cycle::{CycleRunOutcome, CycleStart, GoalCycleDeps, GoalCycleError, GoalCycleRunner};
pub use gate::{GateOutcome, apply_verification_gate};
pub use scheduler::{GoalScheduler, GoalSchedulerDeps, RunRequestAck, RunRequestError, decide_due};
pub use slack::{
    GoalCommand, SlackGoalAlertTriggerHandler, SlackGoalCommandHandler,
    SlackGoalCommandHandlerDeps, parse_goal_command, render_goal_detail, render_goals_overview,
};
pub use store::WorkStateStore;
