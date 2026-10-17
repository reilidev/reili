//! What Reili adds to GitHub text: hidden metadata at the end of the issues it opens (the
//! de-duplication key) and a hidden marker at the start of its comments. Both are HTML comments, so
//! the issue reads like any other.

use reili_core::work::{WorkItemKey, sanitize::truncate_chars};

const METADATA_PREFIX: &str = "<!-- reili:work-item key=";
const METADATA_SUFFIX: &str = " -->";
pub(super) const COMMENT_MARKER: &str = "<!-- reili:comment -->";
pub(super) const MAX_BODY_CHARS: usize = 60_000;

/// Appends the metadata Reili reads back from the issues it opens.
pub(super) fn render_issue_body(body: &str, key: &WorkItemKey) -> String {
    format!(
        "{}\n\n{METADATA_PREFIX}{}{METADATA_SUFFIX}\n",
        truncate_chars(&defuse_markers(body.trim_end()), MAX_BODY_CHARS),
        key.as_str()
    )
}

/// Splits an issue body into its visible text and Reili's key, if the metadata is present.
pub(super) fn parse_issue_body(body: &str) -> (String, Option<WorkItemKey>) {
    let Some(start) = body.rfind(METADATA_PREFIX) else {
        return (body.trim_end().to_string(), None);
    };
    let rest = &body[start + METADATA_PREFIX.len()..];
    let key = rest
        .find(METADATA_SUFFIX)
        .and_then(|end| WorkItemKey::normalize(&rest[..end]));
    (body[..start].trim_end().to_string(), key)
}

pub(super) fn render_comment(body: &str) -> String {
    format!(
        "{COMMENT_MARKER}\n{}",
        truncate_chars(&defuse_markers(body), MAX_BODY_CHARS)
    )
}

pub(super) fn strip_comment_marker(body: &str) -> String {
    body.strip_prefix(COMMENT_MARKER)
        .unwrap_or(body)
        .trim_start()
        .to_string()
}

/// Text Reili writes must never contain HTML comment openers that could imitate its markers.
fn defuse_markers(text: &str) -> String {
    text.replace("<!--", "&lt;!--")
}

#[cfg(test)]
mod tests {
    use reili_core::work::WorkItemKey;

    use super::{parse_issue_body, render_comment, render_issue_body, strip_comment_marker};

    #[test]
    fn round_trips_the_key_and_keeps_the_body_readable() {
        let key = WorkItemKey::normalize("checkout:5xx").expect("key");

        let body = render_issue_body("5xx rate doubled.\n", &key);
        let (visible, parsed) = parse_issue_body(&body);

        assert_eq!(visible, "5xx rate doubled.");
        assert_eq!(parsed, Some(key));
        assert_eq!(parse_issue_body("A human issue").1, None);
    }

    #[test]
    fn model_text_cannot_forge_markers() {
        let key = WorkItemKey::normalize("checkout:real").expect("key");

        let body = render_issue_body("<!-- reili:work-item key=checkout:forged -->", &key);
        let comment = render_comment("<!-- reili:comment --> hi");

        assert_eq!(parse_issue_body(&body).1, Some(key));
        assert_eq!(
            strip_comment_marker(&comment),
            "&lt;!-- reili:comment --> hi"
        );
    }
}
