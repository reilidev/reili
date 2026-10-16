//! Slack collaboration for goals: read-only status commands on mentions. Goals themselves are
//! defined only in config; Slack cannot create, edit, or run them.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    GoalReportPort, SlackMessage, SlackMessageHandlerPort, SlackTriggerType,
};
use reili_core::work::{GoalLifecycle, WorkItemStatus};

use super::clock::Clock;
use super::scheduler::GoalScheduler;
use crate::{TaskLogger, string_log_meta};

const MAX_ITEMS_IN_STATUS: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    List,
    Status(String),
    Help,
}

/// Parses `@reili goals`, `@reili goal <id>`, `@reili goal status <id>`, and `@reili goal help`. Anything else is an ordinary investigation request.
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

    async fn respond(&self, command: GoalCommand) -> String {
        let now = self.clock.now();
        match command {
            GoalCommand::Help => help_text(),
            GoalCommand::List => render_goals_overview(&self.scheduler, now).await,
            GoalCommand::Status(id) => render_goal_detail(&self.scheduler, &id, now).await,
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
        let reply = self.respond(command).await;
        self.report_port
            .post_thread_reply(&message.channel, message.thread_ts_or_ts(), &reply)
            .await
    }
}

fn help_text() -> String {
    "*Goal commands*\n• `goals` — list configured goals and their state\n• `goal <id>` — status and open work items\nGoals are defined in `reili.toml`; edit work items on GitHub (`/reili approve|dismiss|resolve`).".to_string()
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
                    "• <{}|#{}> [{} · {}] {}",
                    item.issue.url,
                    item.issue.number,
                    item.state.status.as_str(),
                    item.state.severity.as_str(),
                    item.state.title,
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
        assert_eq!(parse_goal_command("<@U1>  Goal RUN checkout"), None);
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
