use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::schedule::ObjectiveSchedule;

/// GitHub label names are limited to 50 characters and objectives are labelled `reili-objective:<id>`.
const MAX_OBJECTIVE_ID_LENGTH: usize = 34;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ObjectiveDefinitionError {
    #[error("objective id `{0}` must be 1-34 chars of lowercase letters, digits, `-` or `_`")]
    InvalidId(String),
    #[error("objective `{objective}`: {message}")]
    Invalid { objective: String, message: String },
}

/// Stable, label-safe objective identifier (used in tracker labels).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectiveId(String);

impl ObjectiveId {
    pub fn new(value: impl Into<String>) -> Result<Self, ObjectiveDefinitionError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_OBJECTIVE_ID_LENGTH
            && value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
        if valid {
            Ok(Self(value))
        } else {
            Err(ObjectiveDefinitionError::InvalidId(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectiveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ObjectiveId {
    type Error = ObjectiveDefinitionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ObjectiveId> for String {
    fn from(value: ObjectiveId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveScope {
    pub services: Vec<String>,
    pub repositories: Vec<String>,
    pub environments: Vec<String>,
}

/// Cost limits for one cycle. Cycles run on the objective's schedule, so these also bound the
/// daily cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveBudget {
    pub max_cycle_duration_secs: u64,
    pub max_llm_tokens_per_cycle: u64,
    pub max_tool_calls_per_cycle: u32,
    pub max_new_work_items_per_cycle: u32,
}

impl Default for ObjectiveBudget {
    fn default() -> Self {
        Self {
            max_cycle_duration_secs: 30 * 60,
            max_llm_tokens_per_cycle: 2_000_000,
            max_tool_calls_per_cycle: 200,
            max_new_work_items_per_cycle: 3,
        }
    }
}

/// What an objective is allowed to write. Writes are always limited to the configured tracker
/// project, the objective's report channel, and the configured Canvas; these flags only narrow that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectivePermissions {
    pub save_knowledge: bool,
    pub post_reports: bool,
}

impl Default for ObjectivePermissions {
    fn default() -> Self {
        Self {
            save_knowledge: true,
            post_reports: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveDefinition {
    pub id: ObjectiveId,
    pub description: String,
    pub scope: ObjectiveScope,
    pub schedule: ObjectiveSchedule,
    /// Slack channel ID reports are posted to.
    pub report_channel_id: String,
    pub budget: ObjectiveBudget,
    pub permissions: ObjectivePermissions,
    pub enabled: bool,
}

impl ObjectiveDefinition {
    /// Validates invariants that the config loader cannot express structurally.
    pub fn validate(&self) -> Result<(), ObjectiveDefinitionError> {
        let invalid = |message: &str| ObjectiveDefinitionError::Invalid {
            objective: self.id.to_string(),
            message: message.to_string(),
        };
        if self.description.trim().is_empty() {
            return Err(invalid("description must not be empty"));
        }
        if self.report_channel_id.trim().is_empty() {
            return Err(invalid("report_channel must not be empty"));
        }
        if self.budget.max_cycle_duration_secs == 0
            || self.budget.max_llm_tokens_per_cycle == 0
            || self.budget.max_tool_calls_per_cycle == 0
        {
            return Err(invalid("budget limits must be positive"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ObjectiveDefinitionError, ObjectiveId};
    use crate::work::test_support::sample_objective;

    #[test]
    fn validates_objective_ids() {
        assert!(ObjectiveId::new("checkout-reliability").is_ok());
        assert!(ObjectiveId::new("ci_stability2").is_ok());
        assert_eq!(
            ObjectiveId::new("Checkout"),
            Err(ObjectiveDefinitionError::InvalidId("Checkout".to_string()))
        );
        assert!(ObjectiveId::new("").is_err());
        assert!(ObjectiveId::new("a b").is_err());
        assert!(ObjectiveId::new("x".repeat(34)).is_ok());
        assert!(ObjectiveId::new("x".repeat(35)).is_err());
    }

    #[test]
    fn rejects_empty_budget() {
        let mut objective = sample_objective("checkout");
        objective.budget.max_llm_tokens_per_cycle = 0;

        assert!(objective.validate().is_err());
        assert!(sample_objective("checkout").validate().is_ok());
    }
}
