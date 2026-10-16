//! Resolves and validates goal-driven autonomy settings.

use std::collections::BTreeSet;

use reili_core::goal::{
    AlertTrigger, GoalBudget, GoalDefinition, GoalId, GoalPermissions, GoalReportPolicy,
    GoalSchedule, GoalScope,
};

use super::ConfigError;
use super::env::{EnvironmentReader, read_required_secret};
use super::file::FileConfig;
use super::goal_file::GoalFileConfig;
use super::model::{AutonomyConfig, WorkStateConfig};
use crate::config::SecretString;

const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
const DEFAULT_GITHUB_WEB_BASE_URL: &str = "https://github.com";

fn invalid(field: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError::InvalidValue {
        field: field.into(),
        message: message.into(),
    }
}

pub(super) fn resolve_autonomy(file_config: &FileConfig) -> Result<AutonomyConfig, ConfigError> {
    let autonomy = &file_config.autonomy;
    if autonomy.tick_interval_seconds < 10 {
        return Err(invalid(
            "autonomy.tick_interval_seconds",
            "must be at least 10",
        ));
    }
    if autonomy.max_concurrent_cycles == 0 || autonomy.lease_minutes < 2 {
        return Err(invalid(
            "autonomy",
            "max_concurrent_cycles must be positive and lease_minutes at least 2",
        ));
    }
    Ok(AutonomyConfig {
        tick_interval_seconds: autonomy.tick_interval_seconds,
        max_concurrent_cycles: autonomy.max_concurrent_cycles,
        lease_minutes: autonomy.lease_minutes,
        feedback_poll_minutes: autonomy.feedback_poll_minutes.max(1),
    })
}

pub(super) fn resolve_work_state(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
    goals: &[GoalDefinition],
) -> Result<Option<WorkStateConfig>, ConfigError> {
    let Some(github) = file_config.work_state.github.as_ref() else {
        if goals.is_empty() {
            return Ok(None);
        }
        return Err(invalid(
            "work_state.github",
            "is required when [[goals]] are configured: goals persist their work state as GitHub Issues",
        ));
    };
    let repository = github.repository.trim().to_string();
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    };
    match repository.split_once('/') {
        Some((owner, name)) if valid_part(owner) && valid_part(name) => {}
        _ => {
            return Err(invalid(
                "work_state.github.repository",
                "must be `owner/name` of the dedicated management repository",
            ));
        }
    }
    let app = &file_config.connector.github.app;
    let installation_id = match github.installation_id.as_deref() {
        Some(value) => value
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| {
                invalid(
                    "work_state.github.installation_id",
                    "must be a positive integer string",
                )
            })?,
        None => app.installation_id,
    };
    let app_id = match github.app_id.as_deref() {
        Some(value) if value.trim().parse::<u64>().is_ok_and(|id| id > 0) => {
            value.trim().to_string()
        }
        Some(_) => {
            return Err(invalid(
                "work_state.github.app_id",
                "must be a positive integer string",
            ));
        }
        None => app.app_id.clone(),
    };
    let (private_key_env, field) = match github.private_key_env.as_deref() {
        Some(env_name) => (env_name.to_string(), "work_state.github.private_key_env"),
        None => (
            app.private_key_env.clone(),
            "connector.github.app.private_key_env",
        ),
    };
    let private_key = read_required_secret(env, &private_key_env, field)?;
    Ok(Some(WorkStateConfig {
        repository,
        app_id,
        installation_id,
        private_key: SecretString::new(private_key.expose().replace("\\n", "\n")),
        api_base_url: github
            .api_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE_URL.to_string()),
        web_base_url: github
            .web_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_WEB_BASE_URL.to_string()),
    }))
}

pub(super) fn resolve_goals(file_config: &FileConfig) -> Result<Vec<GoalDefinition>, ConfigError> {
    let mut seen = BTreeSet::new();
    file_config
        .goals
        .iter()
        .enumerate()
        .map(|(index, goal)| {
            let resolved = resolve_goal(goal, index)?;
            if !seen.insert(resolved.id.clone()) {
                return Err(invalid(
                    format!("goals[{index}].id"),
                    format!("duplicate goal id `{}`", resolved.id),
                ));
            }
            Ok(resolved)
        })
        .collect()
}

fn resolve_goal(goal: &GoalFileConfig, index: usize) -> Result<GoalDefinition, ConfigError> {
    let field = |name: &str| format!("goals[{index}].{name}");
    let id =
        GoalId::new(goal.id.trim()).map_err(|error| invalid(field("id"), error.to_string()))?;
    let schedule = GoalSchedule::parse(&goal.schedule, &goal.timezone)
        .map_err(|error| invalid(field("schedule"), error.to_string()))?;
    let channel = goal.report_channel.trim();
    if !(channel.starts_with('C') || channel.starts_with('G')) || channel.len() < 9 {
        return Err(invalid(
            field("report_channel"),
            "must be a Slack channel ID such as C0123456789 (not a channel name)",
        ));
    }
    let defaults = GoalBudget::default();
    let budget = &goal.budget;
    let definition = GoalDefinition {
        id,
        objective: goal.objective.trim().to_string(),
        scope: GoalScope {
            services: goal.scope.clone(),
            repositories: goal.repositories.clone(),
            environments: goal.environments.clone(),
        },
        schedule,
        report: GoalReportPolicy {
            channel_id: channel.to_string(),
            min_interval_minutes: goal.report.min_interval_minutes,
            max_reports_per_day: goal.report.max_reports_per_day,
        },
        budget: GoalBudget {
            max_cycle_duration_secs: budget
                .max_cycle_minutes
                .map_or(defaults.max_cycle_duration_secs, |minutes| {
                    minutes.saturating_mul(60)
                }),
            max_llm_tokens_per_cycle: budget
                .max_llm_tokens_per_cycle
                .unwrap_or(defaults.max_llm_tokens_per_cycle),
            max_llm_tokens_per_day: budget
                .max_llm_tokens_per_day
                .unwrap_or(defaults.max_llm_tokens_per_day),
            max_tool_calls_per_cycle: budget
                .max_tool_calls_per_cycle
                .unwrap_or(defaults.max_tool_calls_per_cycle),
            max_sub_agents_per_cycle: budget
                .max_sub_agents_per_cycle
                .unwrap_or(defaults.max_sub_agents_per_cycle),
            max_parallel_tools: budget
                .max_parallel_tools
                .unwrap_or(defaults.max_parallel_tools),
            max_new_work_items_per_cycle: budget
                .max_new_work_items_per_cycle
                .unwrap_or(defaults.max_new_work_items_per_cycle),
            max_open_work_items: budget
                .max_open_work_items
                .unwrap_or(defaults.max_open_work_items),
            max_cycle_attempts: budget
                .max_cycle_attempts
                .unwrap_or(defaults.max_cycle_attempts),
        },
        permissions: GoalPermissions {
            save_knowledge: goal.permissions.save_knowledge,
            post_reports: goal.permissions.post_reports,
        },
        alert_trigger: goal.alert_trigger.as_ref().map(|trigger| AlertTrigger {
            channel_names: trigger.channels.clone(),
            min_interval_minutes: trigger.min_interval_minutes,
        }),
        enabled: goal.enabled,
    };
    definition
        .validate()
        .map_err(|error| invalid(format!("goals[{index}]"), error.to_string()))?;
    Ok(definition)
}
