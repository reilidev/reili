//! Human collaboration on Work Items: comments from trusted repository members are ingested as
//! feedback, and a small fixed set of `/reili` commands lets humans steer state deterministically.
//! No command can widen permissions, budgets, or scope.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MAX_ARGUMENT_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanComment {
    pub id: u64,
    pub author: String,
    /// True when the author is an owner, member, or collaborator of the management repository.
    pub author_trusted: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanCommand {
    /// The problem is not worth pursuing; closes the work item as not planned.
    Dismiss { reason: String },
    /// The problem is solved; closes the work item as completed.
    Resolve { note: String },
    /// Approves the recommendation without claiming it is implemented yet.
    Approve { note: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHumanComment {
    pub commands: Vec<HumanCommand>,
    /// Remaining free text, used as investigation feedback.
    pub feedback: String,
}

/// Parses `/reili <command> [text]` lines. Unknown commands are kept as feedback text so nothing a
/// human wrote is silently dropped.
#[must_use]
pub fn parse_human_comment(body: &str) -> ParsedHumanComment {
    let mut commands = Vec::new();
    let mut feedback_lines = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("/reili") else {
            feedback_lines.push(line);
            continue;
        };
        let rest = rest.trim();
        let (name, argument) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(name, argument)| (name, argument.trim()));
        let argument: String = argument.chars().take(MAX_ARGUMENT_CHARS).collect();
        let command = match name.to_ascii_lowercase().as_str() {
            "dismiss" => Some(HumanCommand::Dismiss { reason: argument }),
            "resolve" | "resolved" => Some(HumanCommand::Resolve { note: argument }),
            "approve" | "approved" => Some(HumanCommand::Approve { note: argument }),
            "feedback" => {
                feedback_lines.push(line);
                None
            }
            _ => {
                feedback_lines.push(line);
                None
            }
        };
        if let Some(command) = command {
            commands.push(command);
        }
    }

    let feedback = feedback_lines
        .join("\n")
        .replace("/reili feedback", "")
        .trim()
        .to_string();
    ParsedHumanComment { commands, feedback }
}

#[cfg(test)]
mod tests {
    use super::{HumanCommand, parse_human_comment};

    #[test]
    fn parses_commands_and_feedback() {
        let parsed = parse_human_comment(
            "Looks like the cache, not the deploy.\n/reili resolve raised pool size in PR #42\n/reili approve",
        );

        assert_eq!(
            parsed.commands,
            vec![
                HumanCommand::Resolve {
                    note: "raised pool size in PR #42".to_string()
                },
                HumanCommand::Approve {
                    note: String::new()
                },
            ]
        );
        assert_eq!(parsed.feedback, "Looks like the cache, not the deploy.");
    }

    #[test]
    fn keeps_unknown_commands_as_feedback() {
        let parsed = parse_human_comment("/reili grant-write-access to prod");

        assert!(parsed.commands.is_empty());
        assert_eq!(parsed.feedback, "/reili grant-write-access to prod");
    }

    #[test]
    fn parses_bare_commands() {
        let parsed = parse_human_comment("/reili dismiss\n/reili feedback check the CDN too");

        assert_eq!(
            parsed.commands,
            vec![HumanCommand::Dismiss {
                reason: String::new()
            }]
        );
        assert_eq!(parsed.feedback, "check the CDN too");
    }
}
