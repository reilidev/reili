pub fn extract_http_status_code(message: &str) -> Option<u16> {
    let lowered = message.to_ascii_lowercase();
    let prefixes = [
        "status=",
        "status:",
        "status code:",
        "status code =",
        "status_code=",
    ];

    prefixes
        .iter()
        .find_map(|prefix| extract_status_code_after_prefix(&lowered, prefix))
}

fn extract_status_code_after_prefix(message: &str, prefix: &str) -> Option<u16> {
    let start = message.find(prefix)? + prefix.len();
    let rest = &message[start..];
    let trimmed = rest.trim_start();
    let digits: String = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();

    if digits.len() != 3 {
        return None;
    }

    digits.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::extract_http_status_code;

    #[test]
    fn parses_status_equals_pattern() {
        let code = extract_http_status_code(
            "Datadog API request failed: status=429 body=too many requests",
        );

        assert_eq!(code, Some(429));
    }

    #[test]
    fn parses_status_code_colon_pattern() {
        let code = extract_http_status_code(
            "GitHub API responded with status code: 422 Unprocessable Entity",
        );

        assert_eq!(code, Some(422));
    }

    #[test]
    fn returns_none_when_no_status_code_exists() {
        let code = extract_http_status_code("owner is out of scope. allowed owner: acme");

        assert_eq!(code, None);
    }
}
