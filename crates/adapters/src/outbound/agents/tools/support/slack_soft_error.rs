use reili_core::error::PortError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlackToolSoftError {
    pub ok: bool,
    pub kind: String,
    pub message: String,
}

pub fn build_capability_unavailable_soft_error(message: impl Into<String>) -> SlackToolSoftError {
    SlackToolSoftError {
        ok: false,
        kind: "capability_unavailable".to_string(),
        message: message.into(),
    }
}

pub fn to_slack_tool_soft_error(error: &PortError) -> SlackToolSoftError {
    SlackToolSoftError {
        ok: false,
        kind: classify_soft_error_kind(error).to_string(),
        message: error.message.clone(),
    }
}

fn classify_soft_error_kind(error: &PortError) -> &'static str {
    if error.is_invalid_input() || error.is_client_error() {
        return "client_error";
    }

    match error.service_error_code() {
        Some(
            "assistant_search_context_disabled"
            | "feature_not_enabled"
            | "missing_scope"
            | "no_permission"
            | "not_allowed_token_type"
            | "enterprise_is_restricted"
            | "access_denied"
            | "team_access_not_granted",
        ) => "capability_unavailable",
        Some(
            "invalid_action_token"
            | "rate_limited"
            | "ratelimited"
            | "service_unavailable"
            | "internal_error"
            | "fatal_error"
            | "request_timeout",
        ) => "temporary_error",
        Some(_) | None => "temporary_error",
    }
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortError;

    use super::{build_capability_unavailable_soft_error, to_slack_tool_soft_error};

    #[test]
    fn returns_capability_unavailable_for_scope_errors() {
        let error = PortError::service_error(
            "missing_scope",
            "Slack API returned error: method=assistant.search.context error=missing_scope",
        );

        let actual = to_slack_tool_soft_error(&error);

        assert_eq!(
            actual,
            super::SlackToolSoftError {
                ok: false,
                kind: "capability_unavailable".to_string(),
                message:
                    "Slack API returned error: method=assistant.search.context error=missing_scope"
                        .to_string(),
            }
        );
    }

    #[test]
    fn returns_temporary_error_for_action_token_failures() {
        let error = PortError::service_error(
            "invalid_action_token",
            "Slack API returned error: method=assistant.search.context error=invalid_action_token",
        );

        let actual = to_slack_tool_soft_error(&error);

        assert_eq!(actual.kind, "temporary_error");
    }

    #[test]
    fn builds_missing_action_token_error() {
        let actual = build_capability_unavailable_soft_error("missing action token");

        assert_eq!(
            actual,
            super::SlackToolSoftError {
                ok: false,
                kind: "capability_unavailable".to_string(),
                message: "missing action token".to_string(),
            }
        );
    }
}
