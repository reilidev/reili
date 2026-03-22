use reili_core::error::PortError;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatadogToolSoftError {
    pub ok: bool,
    pub kind: String,
    pub status_code: u16,
    pub message: String,
}

pub fn to_datadog_tool_soft_error(error: &PortError) -> Option<DatadogToolSoftError> {
    if !error.is_client_error() {
        return None;
    }
    let status_code = error.status_code()?;

    Some(DatadogToolSoftError {
        ok: false,
        kind: "client_error".to_string(),
        status_code,
        message: error.message.clone(),
    })
}

#[cfg(test)]
mod tests {
    use reili_core::error::PortError;

    use super::to_datadog_tool_soft_error;

    #[test]
    fn returns_soft_error_for_datadog_client_error() {
        let error = PortError::http_status(
            400,
            "Datadog API request failed: status=400 body=bad request",
        );

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
        let error =
            PortError::http_status(500, "Datadog API request failed: status=500 body=failed");

        let actual = to_datadog_tool_soft_error(&error);

        assert_eq!(actual, None);
    }
}
