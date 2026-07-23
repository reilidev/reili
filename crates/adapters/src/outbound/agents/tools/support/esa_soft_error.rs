use reili_core::error::PortError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EsaToolSoftError {
    pub error_type: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_error_code: Option<String>,
}

pub fn to_esa_tool_soft_error(error: &PortError) -> EsaToolSoftError {
    let status_code = error.status_code();
    let (error_type, retryable) = match status_code {
        Some(401 | 403) => ("authorization_error", false),
        Some(404) => ("not_found", false),
        Some(429) => ("rate_limited", true),
        Some(status) if status >= 500 => ("upstream_error", true),
        Some(status) if status >= 400 => ("request_error", false),
        _ if error.is_invalid_input() => ("invalid_input", false),
        _ if error.is_connection_failed() => ("temporary_error", true),
        _ => ("tool_error", false),
    };

    EsaToolSoftError {
        error_type: error_type.to_string(),
        message: error.message.clone(),
        retryable,
        status_code,
        service_error_code: error.service_error_code().map(ToString::to_string),
    }
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortError;

    use super::to_esa_tool_soft_error;

    #[test]
    fn classifies_not_found_as_non_retryable() {
        let error = PortError::http_status(404, "esa API request failed: status=404");

        let actual = to_esa_tool_soft_error(&error);

        assert_eq!(actual.error_type, "not_found");
        assert!(!actual.retryable);
        assert_eq!(actual.status_code, Some(404));
    }

    #[test]
    fn classifies_rate_limited_as_retryable() {
        let error = PortError::http_status(429, "esa API request failed: status=429");

        let actual = to_esa_tool_soft_error(&error);

        assert_eq!(actual.error_type, "rate_limited");
        assert!(actual.retryable);
    }
}
