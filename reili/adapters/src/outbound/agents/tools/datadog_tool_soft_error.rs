use reili_shared::errors::PortError;
use serde::Serialize;

use super::status_code_parser::extract_http_status_code;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogToolSoftError {
    pub ok: bool,
    pub kind: String,
    pub status_code: u16,
    pub message: String,
}

pub fn to_datadog_tool_soft_error(error: &PortError) -> Option<DatadogToolSoftError> {
    let status_code = extract_http_status_code(&error.message)?;
    if !(400..=499).contains(&status_code) {
        return None;
    }

    Some(DatadogToolSoftError {
        ok: false,
        kind: "client_error".to_string(),
        status_code,
        message: error.message.clone(),
    })
}

#[cfg(test)]
mod tests {
    use reili_shared::errors::PortError;

    use super::to_datadog_tool_soft_error;

    #[test]
    fn returns_soft_error_for_datadog_client_error() {
        let error = PortError::new("Datadog API request failed: status=400 body=bad request");

        let actual = to_datadog_tool_soft_error(&error);

        assert_eq!(
            actual,
            Some(super::DatadogToolSoftError {
                ok: false,
                kind: "client_error".to_string(),
                status_code: 400,
                message: "Datadog API request failed: status=400 body=bad request".to_string(),
            }),
        );
    }

    #[test]
    fn returns_none_for_server_error() {
        let error = PortError::new("Datadog API request failed: status=500 body=failed");

        let actual = to_datadog_tool_soft_error(&error);

        assert_eq!(actual, None);
    }
}
