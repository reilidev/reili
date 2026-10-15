use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::fingerprint::stable_fingerprint;
use super::schedule::GoalSchedule;

const MAX_GOAL_ID_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GoalDefinitionError {
    #[error("goal id `{0}` must be 1-64 chars of lowercase letters, digits, `-` or `_`")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricAggregation {
    Avg,
    Max,
    Min,
    Sum,
    Last,
    P95,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetComparator {
    AtMost,
    AtLeast,
}

impl TargetComparator {
    #[must_use]
    pub fn is_met(self, value: f64, target: f64) -> bool {
        match self {
            Self::AtMost => value <= target,
            Self::AtLeast => value >= target,
        }
    }

    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::AtMost => "<=",
            Self::AtLeast => ">=",
        }
    }
}

/// A metric definition evaluated deterministically by Reili (never by the LLM).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionMetric {
    /// Datadog metric query, e.g. `sum:trace.http.request.errors{service:checkout-api}.as_count()`.
    pub query: String,
    pub aggregation: MetricAggregation,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessCriterion {
    pub id: String,
    pub description: String,
    pub metric: Option<CriterionMetric>,
    pub comparator: TargetComparator,
    pub target: f64,
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

/// Early-cycle trigger from alert bot posts in matching Slack channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertTrigger {
    pub channel_names: Vec<String>,
    pub min_interval_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalDefinition {
    pub id: GoalId,
    pub objective: String,
    pub scope: GoalScope,
    pub success_criteria: Vec<SuccessCriterion>,
    pub schedule: GoalSchedule,
    pub report: GoalReportPolicy,
    pub budget: GoalBudget,
    pub permissions: GoalPermissions,
    pub alert_trigger: Option<AlertTrigger>,
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
        let mut seen = std::collections::BTreeSet::new();
        for criterion in &self.success_criteria {
            if criterion.id.trim().is_empty() || !seen.insert(criterion.id.clone()) {
                return Err(invalid("success criteria need unique, non-empty ids"));
            }
            if !criterion.target.is_finite() {
                return Err(invalid("success criterion target must be finite"));
            }
            if let Some(metric) = &criterion.metric
                && (metric.query.trim().is_empty() || metric.window_minutes == 0)
            {
                return Err(invalid(
                    "success criterion metric needs a query and a positive window",
                ));
            }
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
            success_criteria: Vec::new(),
            schedule: GoalSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("schedule"),
            report: GoalReportPolicy {
                channel_id: "C0123".to_string(),
                min_interval_minutes: 60,
                max_reports_per_day: 4,
            },
            budget: GoalBudget::default(),
            permissions: GoalPermissions::default(),
            alert_trigger: None,
            enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::sample_goal;
    use super::{GoalDefinitionError, GoalId, TargetComparator};

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
        assert!(GoalId::new("x".repeat(65)).is_err());
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

    #[test]
    fn comparator_checks_targets() {
        assert!(TargetComparator::AtMost.is_met(0.5, 1.0));
        assert!(!TargetComparator::AtLeast.is_met(0.5, 1.0));
    }
}
