use reili_shared::errors::PortError;
use serde::Serialize;

use super::tool_json::to_json_string;

pub const MAX_DATADOG_TOOL_RESULT_JSON_LENGTH: usize = 20_000;
pub const DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE: &str =
    "Result is too large. Please narrow the time range and try again.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatadogToolResultTooLarge {
    pub ok: bool,
    pub kind: String,
    pub message: String,
}

pub fn serialize_datadog_tool_result_with_size_guard<T>(result: &T) -> Result<String, PortError>
where
    T: Serialize,
{
    serialize_datadog_tool_result_with_size_guard_options(
        result,
        MAX_DATADOG_TOOL_RESULT_JSON_LENGTH,
        DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE,
    )
}

fn serialize_datadog_tool_result_with_size_guard_options<T>(
    result: &T,
    max_json_length: usize,
    too_large_message: &str,
) -> Result<String, PortError>
where
    T: Serialize,
{
    let serialized_result = to_json_string(result)?;
    if serialized_result.chars().count() <= max_json_length {
        return Ok(serialized_result);
    }

    to_json_string(&DatadogToolResultTooLarge {
        ok: false,
        kind: "payload_too_large".to_string(),
        message: too_large_message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::{
        DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE, MAX_DATADOG_TOOL_RESULT_JSON_LENGTH,
        serialize_datadog_tool_result_with_size_guard,
        serialize_datadog_tool_result_with_size_guard_options,
    };

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    struct ToolResultItem {
        id: String,
        payload: String,
    }

    fn create_tool_result(item_count: usize) -> Vec<ToolResultItem> {
        (0..item_count)
            .map(|index| ToolResultItem {
                id: format!("item-{index}"),
                payload: "x".repeat(40),
            })
            .collect()
    }

    #[test]
    fn returns_serialized_payload_when_payload_size_is_within_limit() {
        let result = create_tool_result(5);
        let expected = serde_json::to_string(&result).expect("serialize expected");

        let actual = serialize_datadog_tool_result_with_size_guard(&result).expect("serialize");

        assert!(expected.len() <= MAX_DATADOG_TOOL_RESULT_JSON_LENGTH);
        assert_eq!(actual, expected);
    }

    #[test]
    fn returns_payload_too_large_response_when_payload_size_exceeds_limit() {
        let result = create_tool_result(500);
        let oversized_json = serde_json::to_string(&result).expect("serialize oversized");

        let actual = serialize_datadog_tool_result_with_size_guard(&result).expect("serialize");
        let actual_json: serde_json::Value =
            serde_json::from_str(&actual).expect("deserialize actual");

        assert!(oversized_json.len() > MAX_DATADOG_TOOL_RESULT_JSON_LENGTH);
        assert_eq!(
            actual_json,
            serde_json::json!({
                "ok": false,
                "kind": "payload_too_large",
                "message": DATADOG_TOOL_RESULT_TOO_LARGE_MESSAGE,
            }),
        );
    }

    #[test]
    fn supports_custom_limit_and_message() {
        let result = create_tool_result(1);

        let actual = serialize_datadog_tool_result_with_size_guard_options(
            &result,
            2,
            "custom too large message",
        )
        .expect("serialize");
        let actual_json: serde_json::Value =
            serde_json::from_str(&actual).expect("deserialize actual");

        assert_eq!(
            actual_json,
            serde_json::json!({
                "ok": false,
                "kind": "payload_too_large",
                "message": "custom too large message",
            }),
        );
    }
}
