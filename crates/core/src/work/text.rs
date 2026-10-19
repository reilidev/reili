/// Truncates on a character boundary, marking the cut.
#[must_use]
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("… [truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate_chars("abcdef", 3), "abc… [truncated]");
        assert_eq!(truncate_chars("日本語テキスト", 3), "日本語… [truncated]");
        assert_eq!(truncate_chars("short", 10), "short");
    }
}
