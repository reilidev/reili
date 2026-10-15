//! Slack collaboration for goals: read-only status commands and run requests on mentions, and
//! early-cycle triggers from alert bot posts. Goals themselves are defined only in config;
//! Slack cannot create or edit them.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    GoalReportPort, SlackChannelLookupPort, SlackChannelNamePattern, SlackMessage,
    SlackMessageHandlerPort, SlackTriggerType,
};
use reili_core::work::{CycleTrigger, GoalLifecycle, WorkItemStatus};

use super::clock::Clock;
use super::scheduler::{GoalScheduler, RunRequestAck};
use crate::{TaskLogger, string_log_meta};

const MAX_ITEMS_IN_STATUS: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    List,
    Status(String),
    Run(String),
    Help,
}

/// Parses `@reili goals`, `@reili goal <id>`, `@reili goal status <id>`, `@reili goal run <id>`,
/// and `@reili goal help`. Anything else is an ordinary investigation request.
#[must_use]
pub fn parse_goal_command(text: &str) -> Option<GoalCommand> {
    let cleaned: String = text
        .split_whitespace()
        .filter(|token| !(token.starts_with("<@") && token.ends_with('>')))
        .collect::<Vec<_>>()
        .join(" ");
    let tokens: Vec<String> = cleaned
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    match tokens
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["goals"] | ["goal"] | ["goal", "list"] | ["goals", "list"] => Some(GoalCommand::List),
        ["goal", "help"] | ["goals", "help"] => Some(GoalCommand::Help),
        ["goal", "status", id] | ["goals", "status", id] => {
            Some(GoalCommand::Status((*id).to_string()))
        }
        ["goal", "run", id] | ["goals", "run", id] => Some(GoalCommand::Run((*id).to_string())),
        ["goal", id] if !id.contains(' ') => Some(GoalCommand::Status((*id).to_string())),
        _ => None,
    }
}

pub struct SlackGoalCommandHandler {
    scheduler: Arc<GoalScheduler>,
    report_port: Arc<dyn GoalReportPort>,
    next: Arc<dyn SlackMessageHandlerPort>,
    clock: Arc<dyn Clock>,
    logger: Arc<dyn TaskLogger>,
}

pub struct SlackGoalCommandHandlerDeps {
    pub scheduler: Arc<GoalScheduler>,
    pub report_port: Arc<dyn GoalReportPort>,
    pub next: Arc<dyn SlackMessageHandlerPort>,
    pub clock: Arc<dyn Clock>,
    pub logger: Arc<dyn TaskLogger>,
}

impl SlackGoalCommandHandler {
    pub fn new(deps: SlackGoalCommandHandlerDeps) -> Self {
        Self {
            scheduler: deps.scheduler,
            report_port: deps.report_port,
            next: deps.next,
            clock: deps.clock,
            logger: deps.logger,
        }
    }

    async fn respond(&self, message: &SlackMessage, command: GoalCommand) -> String {
        let now = self.clock.now();
        match command {
            GoalCommand::Help => help_text(),
            GoalCommand::List => render_goals_overview(&self.scheduler, now).await,
            GoalCommand::Status(id) => render_goal_detail(&self.scheduler, &id, now).await,
            GoalCommand::Run(id) => {
                match self
                    .scheduler
                    .request_run(
                        &id,
                        CycleTrigger::SlackRequest {
                            user_id: message.user.clone(),
                        },
                    )
                    .await
                {
                    Ok(RunRequestAck::Queued) => format!(
                        "Queued a cycle for goal `{id}`. It starts within the next scheduler tick; results are posted to the goal's report channel."
                    ),
                    Ok(RunRequestAck::AlreadyQueued) => {
                        format!("A cycle for goal `{id}` is already queued.")
                    }
                    Err(error) => format!("Could not queue goal `{id}`: {error}"),
                }
            }
        }
    }
}

#[async_trait]
impl SlackMessageHandlerPort for SlackGoalCommandHandler {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        let command = (message.trigger == SlackTriggerType::AppMention)
            .then(|| parse_goal_command(&message.text))
            .flatten();
        let Some(command) = command else {
            return self.next.handle(message).await;
        };
        self.logger.info(
            "slack_goal_command",
            string_log_meta([
                ("channel", message.channel.clone()),
                ("user", message.user.clone()),
                ("command", format!("{command:?}")),
            ]),
        );
        let reply = self.respond(&message, command).await;
        self.report_port
            .post_thread_reply(&message.channel, message.thread_ts_or_ts(), &reply)
            .await
    }
}

fn help_text() -> String {
    "*Goal commands*\n• `goals` — list configured goals and their state\n• `goal <id>` — status, open work items, and latest outcomes\n• `goal run <id>` — request an extra cycle now\nGoals are defined in `reili.toml`; edit work items on GitHub (`/reili approve|dismiss|implemented|resolve|rerun`).".to_string()
}

pub async fn render_goals_overview(scheduler: &GoalScheduler, now: DateTime<Utc>) -> String {
    if scheduler.goals().is_empty() {
        return "No goals are configured.".to_string();
    }
    let mut text = "*Goals*\n".to_string();
    for goal in scheduler.goals() {
        let ledger = scheduler
            .store()
            .repository()
            .find_goal_ledger(&goal.id)
            .await;
        let line = match ledger {
            Ok(Some(ledger)) => {
                let open = scheduler
                    .store()
                    .repository()
                    .list_work_items(&goal.id)
                    .await
                    .map(|items| {
                        let open = items
                            .iter()
                            .filter(|item| item.state.status.is_open())
                            .count();
                        let decisions = items
                            .iter()
                            .filter(|item| item.state.status == WorkItemStatus::NeedsHuman)
                            .count();
                        format!("{open} open, {decisions} awaiting decision")
                    })
                    .unwrap_or_else(|error| format!("work items unavailable: {error}"));
                format!(
                    "• `{}` [{}] — {} · last cycle: {} · {} · <{}|ledger>",
                    goal.id,
                    if goal.enabled {
                        ledger.state.lifecycle.as_str()
                    } else {
                        GoalLifecycle::Disabled.as_str()
                    },
                    first_line(&goal.objective),
                    ledger.state.last_cycle().map_or_else(
                        || "never".to_string(),
                        |cycle| format!(
                            "{} ({})",
                            cycle.result.label(),
                            relative(cycle.finished_at, now)
                        )
                    ),
                    open,
                    ledger.issue.url
                )
            }
            Ok(None) => format!("• `{}` — not started yet", goal.id),
            Err(error) => format!("• `{}` — work state unavailable: {error}", goal.id),
        };
        text.push_str(&line);
        text.push('\n');
    }
    text
}

pub async fn render_goal_detail(
    scheduler: &GoalScheduler,
    goal_id: &str,
    now: DateTime<Utc>,
) -> String {
    let Some(goal) = scheduler
        .goals()
        .iter()
        .find(|goal| goal.id.as_str() == goal_id)
    else {
        return format!("Unknown goal `{goal_id}`. Try `goals`.");
    };
    let repository = scheduler.store().repository();
    let ledger = match repository.find_goal_ledger(&goal.id).await {
        Ok(Some(ledger)) => ledger,
        Ok(None) => return format!("Goal `{goal_id}` has not run yet."),
        Err(error) => return format!("Work state for `{goal_id}` is unavailable: {error}"),
    };
    let mut text = format!(
        "*Goal `{}`* ({}) — {}\n",
        goal.id,
        if goal.enabled {
            ledger.state.lifecycle.as_str()
        } else {
            "disabled"
        },
        first_line(&goal.objective)
    );
    if let Some(active) = &ledger.state.active_cycle {
        let _ = writeln!(
            text,
            "Running: `{}` phase {:?}, attempt {} (started {})",
            active.run_id,
            active.phase,
            active.attempt,
            relative(active.started_at, now)
        );
    }
    if let Some(cycle) = ledger.state.last_cycle() {
        let _ = writeln!(
            text,
            "Last cycle: {} ({}) — {}",
            cycle.result.label(),
            relative(cycle.finished_at, now),
            first_line(&cycle.summary)
        );
    }
    let today = now.date_naive();
    let _ = writeln!(
        text,
        "Budget today: {} / {} LLM tokens · {} report(s) posted",
        ledger.state.tokens_used_on(today),
        goal.budget.max_llm_tokens_per_day,
        ledger.state.reports_posted_on(today)
    );
    for history in &ledger.state.criteria {
        if let Some(latest) = history.observations.last() {
            let _ = writeln!(
                text,
                "• criterion `{}`: {}",
                history.criterion_id, latest.detail
            );
        }
    }
    match repository.list_work_items(&goal.id).await {
        Ok(items) => {
            let mut open: Vec<_> = items
                .iter()
                .filter(|item| item.state.status.is_open())
                .collect();
            open.sort_by_key(|item| {
                (
                    item.state.status != WorkItemStatus::NeedsHuman,
                    item.state.severity,
                )
            });
            if open.is_empty() {
                text.push_str("No open work items.\n");
            }
            for item in open.iter().take(MAX_ITEMS_IN_STATUS) {
                let _ = writeln!(
                    text,
                    "• <{}|#{}> [{} · {}] {}{}",
                    item.issue.url,
                    item.issue.number,
                    item.state.status.as_str(),
                    item.state.severity.as_str(),
                    item.state.title,
                    item.state
                        .outcome
                        .as_ref()
                        .and_then(|outcome| outcome.latest_verdict())
                        .map(|verdict| format!(" — outcome {}", verdict.as_str()))
                        .unwrap_or_default()
                );
            }
        }
        Err(error) => {
            let _ = writeln!(text, "Work items unavailable: {error}");
        }
    }
    let _ = write!(text, "<{}|Goal ledger on GitHub>", ledger.issue.url);
    text
}

fn first_line(text: &str) -> String {
    reili_core::work::sanitize::truncate_chars(text.trim().lines().next().unwrap_or_default(), 160)
}

fn relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (now - at).num_minutes();
    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 120 {
        format!("{minutes} min ago")
    } else if minutes < 48 * 60 {
        format!("{} h ago", minutes / 60)
    } else {
        format!("{} d ago", minutes / (24 * 60))
    }
}

/// Requests an early cycle when an alert bot posts in a goal's alert channels. Always passes the
/// message on to `next` (e.g. the auto-response gate) unchanged.
pub struct SlackGoalAlertTriggerHandler {
    scheduler: Arc<GoalScheduler>,
    channel_lookup: Arc<dyn SlackChannelLookupPort>,
    next: Option<Arc<dyn SlackMessageHandlerPort>>,
    logger: Arc<dyn TaskLogger>,
}

impl SlackGoalAlertTriggerHandler {
    pub fn new(
        scheduler: Arc<GoalScheduler>,
        channel_lookup: Arc<dyn SlackChannelLookupPort>,
        next: Option<Arc<dyn SlackMessageHandlerPort>>,
        logger: Arc<dyn TaskLogger>,
    ) -> Self {
        Self {
            scheduler,
            channel_lookup,
            next,
            logger,
        }
    }

    async fn trigger_goals(&self, message: &SlackMessage) {
        let watching: Vec<_> = self
            .scheduler
            .goals()
            .iter()
            .filter(|goal| goal.enabled && goal.alert_trigger.is_some())
            .collect();
        if watching.is_empty() || !message.actor_is_bot || message.thread_ts.is_some() {
            return;
        }
        let Ok(channel) = self
            .channel_lookup
            .lookup_channel_info(&message.channel)
            .await
        else {
            return;
        };
        for goal in watching {
            let Some(alert) = &goal.alert_trigger else {
                continue;
            };
            let matches = alert.channel_names.iter().any(|pattern| {
                SlackChannelNamePattern::new(pattern.clone()).matches(&channel.name)
            });
            if !matches {
                continue;
            }
            let result = self
                .scheduler
                .request_run(
                    goal.id.as_str(),
                    CycleTrigger::Alert {
                        channel_id: message.channel.clone(),
                        message_ts: message.ts.clone(),
                    },
                )
                .await;
            self.logger.info(
                "goal_alert_trigger",
                string_log_meta([
                    ("goalId", goal.id.to_string()),
                    ("channel", message.channel.clone()),
                    (
                        "result",
                        match result {
                            Ok(ack) => format!("{ack:?}"),
                            Err(error) => error.to_string(),
                        },
                    ),
                ]),
            );
        }
    }
}

#[async_trait]
impl SlackMessageHandlerPort for SlackGoalAlertTriggerHandler {
    async fn handle(&self, message: SlackMessage) -> Result<(), PortError> {
        if message.trigger == SlackTriggerType::Message {
            self.trigger_goals(&message).await;
        }
        match &self.next {
            Some(next) => next.handle(message).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GoalCommand, parse_goal_command};

    #[test]
    fn parses_goal_commands() {
        assert_eq!(parse_goal_command("<@U1> goals"), Some(GoalCommand::List));
        assert_eq!(
            parse_goal_command("<@U1> goal checkout-reliability"),
            Some(GoalCommand::Status("checkout-reliability".to_string()))
        );
        assert_eq!(
            parse_goal_command("<@U1>  Goal RUN checkout"),
            Some(GoalCommand::Run("checkout".to_string()))
        );
        assert_eq!(
            parse_goal_command("<@U1> goal help"),
            Some(GoalCommand::Help)
        );
        assert_eq!(parse_goal_command("<@U1> why is checkout slow?"), None);
        assert_eq!(
            parse_goal_command("<@U1> goal of this alert is unclear"),
            None
        );
    }
}
