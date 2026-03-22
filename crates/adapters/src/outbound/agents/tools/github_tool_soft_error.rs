use reili_core::error::PortError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubToolSoftError {
    pub ok: bool,
    pub kind: String,
    pub message: String,
}

pub fn to_github_tool_soft_error(error: &PortError) -> Option<GithubToolSoftError> {
    if error.is_invalid_input() || error.is_client_error() {
        return Some(build_soft_error(error.message.clone()));
    }

    None
}

fn build_soft_error(message: String) -> GithubToolSoftError {
    GithubToolSoftError {
        ok: false,
        kind: "client_error".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortError;

    use super::to_github_tool_soft_error;

    #[test]
    fn returns_soft_error_for_github_api_client_errors() {
        let error = PortError::http_status(
            422,
            "GitHub API request failed: GitHub API responded with status code: 422",
        );

        let actual = to_github_tool_soft_error(&error);

        assert_eq!(
            actual,
            Some(super::GithubToolSoftError {
                ok: false,
                kind: "client_error".to_string(),
                message: "GitHub API request failed: GitHub API responded with status code: 422"
                    .to_string(),
            }),
        );
    }

    #[test]
    fn returns_soft_error_for_scope_validation_errors() {
        let error = PortError::invalid_input("org qualifier is required. include org:example");

        let actual = to_github_tool_soft_error(&error);

        assert_eq!(
            actual,
            Some(super::GithubToolSoftError {
                ok: false,
                kind: "client_error".to_string(),
                message: "org qualifier is required. include org:example".to_string(),
            }),
        );
    }

    #[test]
    fn returns_none_for_non_client_errors() {
        let error = PortError::http_status(
            500,
            "GitHub API request failed: GitHub API responded with status code: 500",
        );

        let actual = to_github_tool_soft_error(&error);

        assert_eq!(actual, None);
    }
}
