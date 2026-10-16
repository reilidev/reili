use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::fingerprint::stable_fingerprint;
use super::schedule::GoalSchedule;

/// GitHub label names are limited to 50 characters and goals are labelled `reili-goal:<id>`.
const MAX_GOAL_ID_LENGTH: usize = 39;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GoalDefinitionError {
    #[error("goal id `{0}` must be 1-39 chars of lowercase letters, digits, `-` or `_`")]
    InvalidId(String),
    #[error("goal `{goal}`: {message}")]
    Invalid { goal: String, message: String },
}

/// Stable, label-safe goal identifier (used in GitHub labels and Slack commands).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GoalId(String);

impl GoalId {
    pub fn new(value: impl Into<String>) -> Result<Self, GoalDefinitionError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_GOAL_ID_LENGTH
            && value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
        if valid {
            Ok(Self(value))
        } else {
            Err(GoalDefinitionError::InvalidId(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GoalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for GoalId {
    type Error = GoalDefinitionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<GoalId> for String {
    fn from(value: GoalId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalScope {
    pub services: Vec<String>,
    pub repositories: Vec<String>,
    pub environments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudget {
    pub max_cycle_duration_secs: u64,
    pub max_llm_tokens_per_cycle: u64,
    pub max_llm_tokens_per_day: u64,
    pub max_tool_calls_per_cycle: u32,
    pub max_sub_agents_per_cycle: u32,
    pub max_parallel_tools: u32,
    pub max_new_work_items_per_cycle: u32,
    pub max_open_work_items: u32,
    pub max_cycle_attempts: u32,
}

impl Default for GoalBudget {
    fn default() -> Self {
        Self {
            max_cycle_duration_secs: 30 * 60,
            max_llm_tokens_per_cycle: 2_000_000,
            max_llm_tokens_per_day: 8_000_000,
            max_tool_calls_per_cycle: 200,
            max_sub_agents_per_cycle: 12,
            max_parallel_tools: 4,
            max_new_work_items_per_cycle: 3,
            max_open_work_items: 20,
            max_cycle_attempts: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalReportPolicy {
    /// Slack channel ID reports are posted to.
    pub channel_id: String,
    pub min_interval_minutes: u32,
    pub max_reports_per_day: u32,
}

/// What a goal is allowed to write. Writes are always limited to the configured management
/// repository, the goal's report channel, and the configured Canvas; these flags only narrow that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPermissions {
    pub save_knowledge: bool,
    pub post_reports: bool,
}

impl Default for GoalPermissions {
    fn default() -> Self {
        Self {
            save_knowledge: true,
            post_reports: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalDefinition {
    pub id: GoalId,
    pub objective: String,
    pub scope: GoalScope,
    pub schedule: GoalSchedule,
    pub report: GoalReportPolicy,
    pub budget: GoalBudget,
    pub permissions: GoalPermissions,
    pub enabled: bool,
}

impl GoalDefinition {
    /// Validates invariants that the config loader cannot express structurally.
    pub fn validate(&self) -> Result<(), GoalDefinitionError> {
        let invalid = |message: &str| GoalDefinitionError::Invalid {
            goal: self.id.to_string(),
            message: message.to_string(),
        };
        if self.objective.trim().is_empty() {
            return Err(invalid("objective must not be empty"));
        }
        if self.report.channel_id.trim().is_empty() {
            return Err(invalid("report_channel must not be empty"));
        }
        if self.budget.max_cycle_duration_secs == 0
            || self.budget.max_llm_tokens_per_cycle == 0
            || self.budget.max_tool_calls_per_cycle == 0
            || self.budget.max_parallel_tools == 0
            || self.budget.max_cycle_attempts == 0
        {
            return Err(invalid("budget limits must be positive"));
        }
        if self.budget.max_llm_tokens_per_cycle > self.budget.max_llm_tokens_per_day {
            return Err(invalid(
                "max_llm_tokens_per_cycle must not exceed max_llm_tokens_per_day",
            ));
        }
        Ok(())
    }

    /// Fingerprint of everything that changes what the goal asks for. Persisted in the goal
    /// ledger so definition changes are detected across restarts.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        stable_fingerprint(&canonical)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{GoalBudget, GoalDefinition, GoalId, GoalPermissions, GoalReportPolicy, GoalScope};
    use crate::goal::GoalSchedule;

    pub(crate) fn sample_goal(id: &str) -> GoalDefinition {
        GoalDefinition {
            id: GoalId::new(id).expect("goal id"),
            objective: "Improve checkout reliability".to_string(),
            scope: GoalScope {
                services: vec!["checkout-api".to_string()],
                repositories: vec!["acme/checkout".to_string()],
                environments: vec!["production".to_string()],
            },
            schedule: GoalSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("schedule"),
            report: GoalReportPolicy {
                channel_id: "C0123".to_string(),
                min_interval_minutes: 60,
                max_reports_per_day: 4,
            },
            budget: GoalBudget::default(),
            permissions: GoalPermissions::default(),
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::sample_goal;
    use super::{GoalDefinitionError, GoalId};

    #[test]
    fn validates_goal_ids() {
        assert!(GoalId::new("checkout-reliability").is_ok());
        assert!(GoalId::new("ci_stability2").is_ok());
        assert_eq!(
            GoalId::new("Checkout"),
            Err(GoalDefinitionError::InvalidId("Checkout".to_string()))
        );
        assert!(GoalId::new("").is_err());
        assert!(GoalId::new("a b").is_err());
        assert!(GoalId::new("x".repeat(39)).is_ok());
        assert!(GoalId::new("x".repeat(40)).is_err());
    }

    #[test]
    fn fingerprint_changes_with_definition() {
        let goal = sample_goal("checkout");
        let mut changed = goal.clone();
        changed.objective = "Reduce checkout latency".to_string();

        assert_eq!(goal.fingerprint(), sample_goal("checkout").fingerprint());
        assert_ne!(goal.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn rejects_inconsistent_budget() {
        let mut goal = sample_goal("checkout");
        goal.budget.max_llm_tokens_per_cycle = goal.budget.max_llm_tokens_per_day + 1;

        assert!(goal.validate().is_err());
        assert!(sample_goal("checkout").validate().is_ok());
    }
}
