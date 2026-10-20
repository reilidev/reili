use serde::Deserialize;

use super::file::optional_non_empty_string;

fn default_true() -> bool {
    true
}

fn default_timezone() -> String {
    "UTC".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
/// One optional section per supported Work State backend; exactly one may be set.
pub(crate) struct WorkStateFileConfig {
    pub github: Option<WorkStateGitHubFileConfig>,
}

/// The repository the team tracks its work in; Reili reads and writes only there. `connection` names the
/// `[connector.github.connection.<name>]` it authenticates with (optional only while a single
/// connection is defined); tokens are always down-scoped to this repository and to `issues: write` +
/// `metadata: read`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkStateGitHubFileConfig {
    pub repository: String,
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub connection: Option<String>,
    pub api_base_url: Option<String>,
    pub web_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectiveFileConfig {
    pub id: String,
    pub description: String,
    pub schedule: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    pub report_channel: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub budget: ObjectiveBudgetFileConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ObjectiveBudgetFileConfig {
    pub max_cycle_minutes: Option<u64>,
    pub max_llm_tokens_per_cycle: Option<u64>,
    pub max_tool_calls_per_cycle: Option<u32>,
    pub max_new_work_items_per_cycle: Option<u32>,
}
