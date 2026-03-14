use reili_core::error::PortError;
use serde::Serialize;

use super::status_code_parser::extract_http_status_code;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubToolSoftError {
    pub ok: bool,
    pub kind: String,
    pub message: String,
}

pub fn to_github_tool_soft_error(error: &PortError) -> Option<GithubToolSoftError> {
    if is_local_input_validation_error(&error.message) {
        return Some(build_soft_error(error.message.clone()));
    }

    let status_code = extract_http_status_code(&error.message)?;
    if (400..=499).contains(&status_code) {
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

fn is_local_input_validation_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();

    lowered.contains("org qualifier is required")
        || lowered.contains("org qualifier is out of scope")
        || lowered.contains("owner is out of scope")
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortError;

    use super::to_github_tool_soft_error;

    #[test]
    fn returns_soft_error_for_github_api_client_errors() {
        let error =
            PortError::new("GitHub API request failed: GitHub API responded with status code: 422");

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
        let error = PortError::new("org qualifier is required. include org:example");

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
        let error =
            PortError::new("GitHub API request failed: GitHub API responded with status code: 500");

        let actual = to_github_tool_soft_error(&error);

        assert_eq!(actual, None);
    }
}
