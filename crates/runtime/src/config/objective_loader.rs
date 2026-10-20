use std::collections::BTreeSet;

use reili_core::objective::{ObjectiveBudget, ObjectiveDefinition, ObjectiveId, ObjectiveSchedule};

use reili_core::work::RepositoryName;

use super::ConfigError;
use super::env::EnvironmentReader;
use super::file::FileConfig;
use super::github_connection::{GitHubConnectionReference, resolve_github_connection};
use super::model::{GitHubWorkStateBackendConfig, WorkStateConfig};
use super::objective_file::{ObjectiveFileConfig, WorkStateFileConfig, WorkStateGitHubFileConfig};

const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
const DEFAULT_GITHUB_WEB_BASE_URL: &str = "https://github.com";

fn invalid(field: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError::InvalidValue {
        field: field.into(),
        message: message.into(),
    }
}

pub(super) fn resolve_work_state(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
    objectives: &[ObjectiveDefinition],
) -> Result<Option<WorkStateConfig>, ConfigError> {
    let WorkStateFileConfig { github } = &file_config.work_state;
    match github {
        Some(github) => Ok(Some(WorkStateConfig::GitHub(resolve_github_work_state(
            file_config,
            env,
            github,
        )?))),
        None if objectives.is_empty() => Ok(None),
        None => Err(invalid(
            "work_state",
            "needs exactly one backend section when [[objectives]] are configured (supported: [work_state.github])",
        )),
    }
}

fn resolve_github_work_state(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
    github: &WorkStateGitHubFileConfig,
) -> Result<GitHubWorkStateBackendConfig, ConfigError> {
    let repository: RepositoryName = github.repository.parse().map_err(|_| {
        invalid(
            "work_state.github.repository",
            "must be `owner/name` of the repository the team tracks its work in",
        )
    })?;
    let credentials = resolve_github_connection(
        &GitHubConnectionReference {
            connections: &file_config.connector.github.connection,
            selected: github.connection.as_deref(),
            selector_field: "work_state.github.connection",
        },
        env,
    )?;
    Ok(GitHubWorkStateBackendConfig {
        repository,
        app_id: credentials.app_id,
        installation_id: credentials.installation_id,
        private_key: credentials.private_key,
        api_base_url: github
            .api_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_string()),
        web_base_url: github
            .web_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_WEB_BASE_URL.to_string()),
    })
}

pub(super) fn resolve_objectives(
    file_config: &FileConfig,
) -> Result<Vec<ObjectiveDefinition>, ConfigError> {
    let mut seen = BTreeSet::new();
    let mut objectives = file_config
        .objectives
        .iter()
        .enumerate()
        .map(|(index, objective)| {
            let resolved = resolve_objective(objective, index)?;
            if !seen.insert(resolved.id.clone()) {
                return Err(invalid(
                    format!("objectives[{index}].id"),
                    format!("duplicate objective id `{}`", resolved.id),
                ));
            }
            Ok(resolved)
        })
        .collect::<Result<Vec<_>, _>>()?;
    resolve_default_objective(&mut objectives)?;
    Ok(objectives)
}

fn resolve_default_objective(objectives: &mut [ObjectiveDefinition]) -> Result<(), ConfigError> {
    let explicit: Vec<usize> = objectives
        .iter()
        .enumerate()
        .filter(|(_, objective)| objective.takes_unlinked_work)
        .map(|(index, _)| index)
        .collect();
    match explicit.as_slice() {
        [] => {
            let mut enabled = objectives.iter_mut().filter(|objective| objective.enabled);
            if let (Some(only), None) = (enabled.next(), enabled.next()) {
                only.takes_unlinked_work = true;
            }
            Ok(())
        }
        [index] if objectives[*index].enabled => Ok(()),
        [index] => Err(invalid(
            format!("objectives[{index}].default"),
            "the default objective must be enabled",
        )),
        [_, second, ..] => Err(invalid(
            format!("objectives[{second}].default"),
            "only one objective can be the default",
        )),
    }
}

fn resolve_objective(
    objective: &ObjectiveFileConfig,
    index: usize,
) -> Result<ObjectiveDefinition, ConfigError> {
    let field = |name: &str| format!("objectives[{index}].{name}");
    let id = ObjectiveId::new(objective.id.trim())
        .map_err(|error| invalid(field("id"), error.to_string()))?;
    let schedule = ObjectiveSchedule::parse(&objective.schedule, &objective.timezone)
        .map_err(|error| invalid(field("schedule"), error.to_string()))?;
    let channel = objective.report_channel.trim();
    if !(channel.starts_with('C') || channel.starts_with('G')) || channel.len() < 9 {
        return Err(invalid(
            field("report_channel"),
            "must be a Slack channel ID such as C0123456789 (not a channel name)",
        ));
    }
    let defaults = ObjectiveBudget::default();
    let budget = &objective.budget;
    let definition = ObjectiveDefinition {
        id,
        description: objective.description.trim().to_string(),
        schedule,
        report_channel_id: channel.to_string(),
        budget: ObjectiveBudget {
            max_cycle_duration_secs: budget
                .max_cycle_minutes
                .map_or(defaults.max_cycle_duration_secs, |minutes| {
                    minutes.saturating_mul(60)
                }),
            max_llm_tokens_per_cycle: budget
                .max_llm_tokens_per_cycle
                .unwrap_or(defaults.max_llm_tokens_per_cycle),
            max_tool_calls_per_cycle: budget
                .max_tool_calls_per_cycle
                .unwrap_or(defaults.max_tool_calls_per_cycle),
            max_new_work_items_per_cycle: budget
                .max_new_work_items_per_cycle
                .unwrap_or(defaults.max_new_work_items_per_cycle),
        },
        enabled: objective.enabled,
        takes_unlinked_work: objective.default,
    };
    definition
        .validate()
        .map_err(|error| invalid(format!("objectives[{index}]"), error.to_string()))?;
    Ok(definition)
}
