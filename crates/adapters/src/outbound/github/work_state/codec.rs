//! Issue body format: a human-readable rendering of the state, a human-notes section Reili
//! preserves verbatim, and a machine-readable JSON block that is the source of truth for Reili.

use std::fmt::Write as _;

use reili_core::work::{
    ObjectiveLedgerState, WorkItemState, WorkItemStatus, sanitize::truncate_chars,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub(super) const MAX_BODY_CHARS: usize = 65_000;
const STATE_BEGIN: &str = "<!-- reili:state:begin -->";
const STATE_END: &str = "<!-- reili:state:end -->";
const NOTES_BEGIN: &str = "<!-- reili:human-notes:begin -->";
const NOTES_END: &str = "<!-- reili:human-notes:end -->";
pub(super) const COMMENT_MARKER_PREFIX: &str = "<!-- reili:comment";
const MAX_HUMAN_NOTES_CHARS: usize = 8_000;
const DEFAULT_NOTES: &str = "_Add context for Reili here (owners, constraints, links). Reili preserves this section and reads it as guidance._";

/// Serializes state as JSON safe to embed in an HTML comment region: `<` is escaped so no string
/// value can close the marker comments.
pub(super) fn encode_state<T: Serialize>(state: &T) -> Result<String, String> {
    serde_json::to_string_pretty(state)
        .map(|json| json.replace('<', "\\u003c"))
        .map_err(|error| error.to_string())
}

pub(super) fn decode_state<T: DeserializeOwned>(body: &str) -> Result<T, String> {
    // The state block is always last, so text earlier in the body that happens to contain the
    // marker (model output, human edits) cannot shadow it.
    let start = body
        .rfind(STATE_BEGIN)
        .ok_or_else(|| "machine state block is missing".to_string())?
        + STATE_BEGIN.len();
    let end = body[start..]
        .find(STATE_END)
        .ok_or_else(|| "machine state block is not terminated".to_string())?
        + start;
    let block = body[start..end].trim();
    let json = block
        .strip_prefix("```json")
        .and_then(|rest| rest.strip_suffix("```"))
        .unwrap_or(block)
        .trim();
    serde_json::from_str(json).map_err(|error| format!("machine state is not valid JSON: {error}"))
}

pub(super) fn extract_human_notes(body: &str) -> String {
    let Some(start) = body.find(NOTES_BEGIN) else {
        return String::new();
    };
    let start = start + NOTES_BEGIN.len();
    let Some(end) = body[start..].find(NOTES_END) else {
        return String::new();
    };
    let notes = body[start..start + end].trim();
    if notes == DEFAULT_NOTES {
        String::new()
    } else {
        truncate_chars(notes, MAX_HUMAN_NOTES_CHARS)
    }
}

fn notes_and_state(human_notes: &str, state_json: &str) -> String {
    let notes = if human_notes.trim().is_empty() {
        DEFAULT_NOTES.to_string()
    } else {
        truncate_chars(
            &human_notes.trim().replace("<!-- reili:", "&lt;!-- reili:"),
            MAX_HUMAN_NOTES_CHARS,
        )
    };
    format!(
        "\n## Human notes\n{NOTES_BEGIN}\n{notes}\n{NOTES_END}\n\n<details><summary>Reili state (machine-readable; do not edit)</summary>\n\n{STATE_BEGIN}\n```json\n{state_json}\n```\n{STATE_END}\n</details>\n"
    )
}

/// Rendered text must never contain HTML comment openers that could imitate Reili's markers.
fn defuse_markers(text: &str) -> String {
    text.replace("<!--", "&lt;!--")
}

fn cell(value: &str) -> String {
    truncate_chars(value, 300)
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
}

pub(super) fn render_work_item_body(
    state: &WorkItemState,
    human_notes: &str,
) -> Result<String, String> {
    let state_json = encode_state(state)?;
    let full = defuse_markers(&render_work_item_sections(state, true));
    let mut body = format!("{full}{}", notes_and_state(human_notes, &state_json));
    if body.chars().count() > MAX_BODY_CHARS {
        let compact = defuse_markers(&render_work_item_sections(state, false));
        body = format!("{compact}{}", notes_and_state(human_notes, &state_json));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!(
            "work item body is {} characters, above GitHub's limit",
            body.chars().count()
        ));
    }
    Ok(body)
}

fn render_work_item_sections(state: &WorkItemState, detailed: bool) -> String {
    let mut text = format!(
        "> Maintained by Reili for objective `{}`. Edit only **Human notes** below. Steer with a comment (read by the next cycle), the `reili-status:*` label, or by closing the issue.\n\n**Status:** `{}` · **Severity:** {} · **Kind:** {} · **Key:** `{}`\n",
        state.objective_id,
        state.status.as_str(),
        state.severity.as_str(),
        state.kind.as_str(),
        state.key,
    );
    if let Some(decision) = &state.human_decision {
        let _ = write!(text, "\n### Decision needed\n{}\n", decision.question);
    }
    let _ = write!(text, "\n### Problem\n{}\n", state.problem_statement);
    if !state.hypotheses.is_empty() {
        text.push_str(
            "\n### Hypotheses\n| ID | Status | Hypothesis | Evidence |\n|---|---|---|---|\n",
        );
        for hypothesis in &state.hypotheses {
            let _ = writeln!(
                text,
                "| {} | {} | {} | {} |",
                cell(&hypothesis.id),
                hypothesis.status.as_str(),
                cell(&hypothesis.statement),
                cell(&hypothesis.evidence_ids.join(", "))
            );
        }
    }
    if !state.findings.is_empty() {
        text.push_str("\n### Findings\n");
        for finding in &state.findings {
            let _ = writeln!(
                text,
                "- {}{}",
                finding.statement,
                if finding.evidence_ids.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", finding.evidence_ids.join(", "))
                }
            );
        }
    }
    if let Some(recommendation) = &state.recommendation {
        let _ = write!(text, "\n### Recommendation\n{}\n", recommendation.action);
        if detailed {
            for (label, value) in [
                ("Rationale", &recommendation.rationale),
                ("Expected effect", &recommendation.expected_effect),
                ("Risk", &recommendation.risk),
            ] {
                if !value.trim().is_empty() {
                    let _ = writeln!(text, "- {label}: {value}");
                }
            }
        }
    }
    for (title, values) in [
        ("Open questions", &state.open_questions),
        ("Next actions", &state.next_actions),
        ("Investigation plan", &state.plan),
    ] {
        if !values.is_empty() {
            let _ = write!(text, "\n### {title}\n");
            for value in values {
                let _ = writeln!(text, "- {value}");
            }
        }
    }
    if !state.evidence.is_empty() {
        text.push_str("\n### Evidence\n");
        for evidence in &state.evidence {
            let _ = writeln!(
                text,
                "- `{}` {}{}{}",
                evidence.id,
                evidence.description,
                evidence
                    .url
                    .as_deref()
                    .map(|url| format!(" — {url}"))
                    .unwrap_or_default(),
                if detailed {
                    evidence
                        .query
                        .as_deref()
                        .map(|query| format!(" (query: `{}`)", cell(query)))
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            );
        }
    }
    if detailed && !state.history.is_empty() {
        text.push_str("\n### History\n");
        for change in state.history.iter().rev().take(10) {
            let _ = writeln!(
                text,
                "- {} `{}` → `{}` by {} — {}",
                change.at.to_rfc3339(),
                change.from.map_or("new", WorkItemStatus::as_str),
                change.to.as_str(),
                match &change.actor {
                    reili_core::work::StatusActor::Reili => "Reili".to_string(),
                    reili_core::work::StatusActor::Human { login } => format!("@{login}"),
                },
                cell(&change.reason)
            );
        }
    }
    text
}

pub(super) fn render_ledger_body(
    state: &ObjectiveLedgerState,
    human_notes: &str,
) -> Result<String, String> {
    let state_json = encode_state(state)?;
    let mut text = format!(
        "> Objective ledger maintained by Reili. The objective itself is defined in `reili.toml`; edit only **Human notes** below.\n\n**Objective:** `{}` · **Lifecycle:** `{}`\n\n### Objective\n{}\n",
        state.objective_id,
        state.lifecycle.as_str(),
        state.description
    );
    if let Some(active) = &state.active_cycle {
        let _ = write!(
            text,
            "\n### Running cycle\n`{}` — phase {:?}, attempt {}, lease until {}\n",
            active.run_id,
            active.phase,
            active.attempt,
            active.lease_expires_at.to_rfc3339()
        );
    }
    if !state.cycles.is_empty() {
        text.push_str(
            "\n### Recent cycles\n| Run | Finished | Result | Tokens |\n|---|---|---|---|\n",
        );
        for cycle in state.cycles.iter().rev().take(10) {
            let _ = writeln!(
                text,
                "| `{}` | {} | {} | {} |",
                cycle.run_id,
                cycle.finished_at.to_rfc3339(),
                cell(&cycle.result.label()),
                cycle.llm_tokens
            );
        }
    }
    if let Some(usage) = &state.usage {
        let _ = write!(
            text,
            "\n### Usage on {}\n{} LLM tokens · {} cycle(s) · {} report(s)\n",
            usage.day, usage.llm_tokens, usage.cycles, usage.reports_posted
        );
    }
    let body = format!(
        "{}{}",
        defuse_markers(&text),
        notes_and_state(human_notes, &state_json)
    );
    if body.chars().count() > MAX_BODY_CHARS {
        return Err("objective ledger body exceeds GitHub's limit".to_string());
    }
    Ok(body)
}

pub(super) fn render_comment(kind: &str, body: &str) -> String {
    format!(
        "{COMMENT_MARKER_PREFIX} kind={kind} -->\n{}",
        truncate_chars(body, 20_000)
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use reili_core::work::{
        Finding, Hypothesis, HypothesisStatus, ObjectiveLedgerState, WorkItemState,
        test_support::sample_work_item,
    };

    use super::{
        decode_state, encode_state, extract_human_notes, render_ledger_body, render_work_item_body,
    };

    fn item() -> WorkItemState {
        let mut state = sample_work_item("checkout:5xx");
        state.hypotheses.push(Hypothesis {
            id: "H1".to_string(),
            statement:
                "pool | exhaustion <!-- reili:state:begin --> {} <!-- reili:state:end --> injected"
                    .to_string(),
            status: HypothesisStatus::Supported,
            evidence_ids: vec!["r1-E1".to_string()],
            rationale: String::new(),
        });
        state.findings.push(Finding {
            statement: "errors rose".to_string(),
            evidence_ids: vec!["r1-E1".to_string()],
        });
        state
    }

    #[test]
    fn round_trips_state_even_with_marker_text_inside_values() {
        let state = item();

        let body = render_work_item_body(&state, "Owner: team-payments").expect("render");
        let decoded: WorkItemState = decode_state(&body).expect("decode");

        assert_eq!(decoded, state);
        assert_eq!(extract_human_notes(&body), "Owner: team-payments");
        assert!(body.contains("- errors rose (r1-E1)"));
        assert!(!body[..body.rfind("<details>").expect("details")].contains("<!-- reili:state"));
        assert!(body.contains("| H1 | supported |"));
    }

    #[test]
    fn default_notes_are_not_reported_as_human_notes() {
        let body = render_work_item_body(&item(), "").expect("render");

        assert_eq!(extract_human_notes(&body), "");
    }

    #[test]
    fn reports_corrupted_state() {
        let mut body = render_work_item_body(&item(), "").expect("render");
        body = body.replace("\"schema_version\"", "\"schema_version\" oops");

        let error = decode_state::<WorkItemState>(&body).expect_err("corrupted");
        assert!(error.contains("not valid JSON"));
        assert!(decode_state::<WorkItemState>("plain text").is_err());
    }

    #[test]
    fn encodes_angle_brackets() {
        let json = encode_state(&item()).expect("encode");

        assert!(!json.contains("<!--"));
    }

    #[test]
    fn renders_ledger() {
        let objective = crate::outbound::github::work_state::tests_support::objective();
        let ledger = ObjectiveLedgerState::new(
            &objective,
            Utc.timestamp_opt(1_760_000_000, 0).single().expect("time"),
        );

        let body = render_ledger_body(&ledger, "notes").expect("render");
        let decoded: ObjectiveLedgerState = decode_state(&body).expect("decode");

        assert_eq!(decoded, ledger);
        assert!(body.contains("**Lifecycle:** `active`"));
    }
}
