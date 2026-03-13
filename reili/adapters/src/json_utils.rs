use serde_json::Value;

const MAX_ERROR_BODY_PREVIEW_CHARS: usize = 1_000;

pub(crate) fn read_non_empty_json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn read_non_empty_string(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(crate) fn truncate_for_error(value: &str) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(MAX_ERROR_BODY_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        return format!("{preview}...");
    }

    preview
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{read_non_empty_json_string, read_non_empty_string, truncate_for_error};

    #[test]
    fn trims_and_filters_owned_strings() {
        assert_eq!(
            read_non_empty_string(Some("  value  ".to_string())),
            Some("value".to_string())
        );
        assert_eq!(read_non_empty_string(Some("   ".to_string())), None);
        assert_eq!(read_non_empty_string(None), None);
    }

    #[test]
    fn trims_and_filters_json_strings() {
        let value = json!("  value  ");
        let empty = json!(" ");
        let number = json!(1);

        assert_eq!(
            read_non_empty_json_string(Some(&value)),
            Some("value".to_string())
        );
        assert_eq!(read_non_empty_json_string(Some(&empty)), None);
        assert_eq!(read_non_empty_json_string(Some(&number)), None);
        assert_eq!(read_non_empty_json_string(None), None);
    }

    #[test]
    fn truncates_error_preview_only_when_input_exceeds_limit() {
        let short = "value";
        let long = "x".repeat(1_001);

        assert_eq!(truncate_for_error(short), "value".to_string());
        assert_eq!(
            truncate_for_error(&long),
            format!("{}...", "x".repeat(1_000))
        );
    }
}
