//! TOML schema for goal-driven autonomy: `[work_state.github]`, `[autonomy]`, and `[[goals]]`.

use serde::Deserialize;

fn default_true() -> bool {
    true
}

fn default_timezone() -> String {
    "UTC".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WorkStateFileConfig {
    pub github: Option<WorkStateGitHubFileConfig>,
}

/// Dedicated management repository for Work State. App credentials default to
/// `[connector.github.app]`; tokens are always down-scoped to this repository and to
/// `issues: write` + `metadata: read`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkStateGitHubFileConfig {
    pub repository: String,
    pub app_id: Option<String>,
    pub installation_id: Option<String>,
    pub private_key_env: Option<String>,
    pub api_base_url: Option<String>,
    pub web_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AutonomyFileConfig {
    pub tick_interval_seconds: u64,
    pub max_concurrent_cycles: u32,
    pub lease_minutes: u32,
    pub feedback_poll_minutes: u32,
    /// Direct Datadog metric queries for goal success criteria and outcome tracking.
    pub metric_queries: bool,
}

impl Default for AutonomyFileConfig {
    fn default() -> Self {
        Self {
            tick_interval_seconds: 60,
            max_concurrent_cycles: 1,
            lease_minutes: 15,
            feedback_poll_minutes: 10,
            metric_queries: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalFileConfig {
    pub id: String,
    pub objective: String,
    /// Services in scope (e.g. Datadog service names).
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub repositories: Vec<String>,
    #[serde(default)]
    pub environments: Vec<String>,
    pub schedule: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    pub report_channel: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub report: GoalReportFileConfig,
    #[serde(default)]
    pub budget: GoalBudgetFileConfig,
    #[serde(default)]
    pub permissions: GoalPermissionsFileConfig,
    #[serde(default)]
    pub alert_trigger: Option<GoalAlertTriggerFileConfig>,
    #[serde(default)]
    pub success_criteria: Vec<SuccessCriterionFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GoalReportFileConfig {
    pub min_interval_minutes: u32,
    pub max_reports_per_day: u32,
}

impl Default for GoalReportFileConfig {
    fn default() -> Self {
        Self {
            min_interval_minutes: 60,
            max_reports_per_day: 6,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GoalBudgetFileConfig {
    pub max_cycle_minutes: Option<u64>,
    pub max_llm_tokens_per_cycle: Option<u64>,
    pub max_llm_tokens_per_day: Option<u64>,
    pub max_tool_calls_per_cycle: Option<u32>,
    pub max_sub_agents_per_cycle: Option<u32>,
    pub max_parallel_tools: Option<u32>,
    pub max_new_work_items_per_cycle: Option<u32>,
    pub max_open_work_items: Option<u32>,
    pub max_cycle_attempts: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GoalPermissionsFileConfig {
    pub save_knowledge: bool,
    pub post_reports: bool,
}

impl Default for GoalPermissionsFileConfig {
    fn default() -> Self {
        Self {
            save_knowledge: true,
            post_reports: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoalAlertTriggerFileConfig {
    pub channels: Vec<String>,
    #[serde(default = "default_alert_interval")]
    pub min_interval_minutes: u32,
}

fn default_alert_interval() -> u32 {
    60
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuccessCriterionFileConfig {
    pub id: String,
    pub description: String,
    pub metric_query: Option<String>,
    #[serde(default = "default_aggregation")]
    pub aggregation: String,
    #[serde(default = "default_window")]
    pub window_minutes: u32,
    pub comparator: String,
    pub target: f64,
}

fn default_aggregation() -> String {
    "avg".to_string()
}

fn default_window() -> u32 {
    60
}
