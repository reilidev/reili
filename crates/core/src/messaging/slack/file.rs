use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SlackMessageFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, alias = "plain_text", skip_serializing_if = "Option::is_none")]
    pub plain_text: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_binary: bool,
}

impl SlackMessageFile {
    pub fn rendered_text(&self) -> Option<String> {
        let name = self.name().unwrap_or_default();
        let plain_text = self.plain_text().unwrap_or_default();

        if name.is_empty() && plain_text.is_empty() && !self.is_binary {
            return None;
        }

        let mut parts = vec![format!("## Attached file title\n {name}\n")];
        if self.is_binary {
            parts.push("This is binary file".to_string());
        }
        if !plain_text.is_empty() || !self.is_binary {
            parts.push(format!("## Plain text\n{plain_text}\n"));
        }

        Some(parts.join("\n"))
    }

    fn name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .filter(|text| !text.is_empty())
            .or_else(|| self.title.as_deref().filter(|text| !text.is_empty()))
    }

    fn plain_text(&self) -> Option<&str> {
        self.plain_text.as_deref().filter(|text| !text.is_empty())
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn render_slack_message_files_text(files: &[SlackMessageFile]) -> Option<String> {
    let parts: Vec<String> = files
        .iter()
        .filter_map(SlackMessageFile::rendered_text)
        .collect();

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::{SlackMessageFile, render_slack_message_files_text};

    #[test]
    fn renders_file_name_and_plain_text() {
        let file = SlackMessageFile {
            name: Some("alert.eml".to_string()),
            title: Some("Alert email".to_string()),
            plain_text: Some("scheduled upgrade required".to_string()),
            is_binary: false,
        };

        assert_eq!(
            file.rendered_text(),
            Some(
                "## Attached file title\n alert.eml\n\n## Plain text\nscheduled upgrade required\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn falls_back_to_title_when_name_is_missing() {
        let file = SlackMessageFile {
            name: None,
            title: Some("AWS Health Event".to_string()),
            plain_text: Some("important notice".to_string()),
            is_binary: false,
        };

        assert_eq!(
            file.rendered_text(),
            Some(
                "## Attached file title\n AWS Health Event\n\n## Plain text\nimportant notice\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn renders_empty_plain_text_when_plain_text_is_missing() {
        let file = SlackMessageFile {
            name: Some("alert.eml".to_string()),
            title: Some("Alert email".to_string()),
            plain_text: None,
            is_binary: false,
        };

        assert_eq!(
            file.rendered_text(),
            Some("## Attached file title\n alert.eml\n\n## Plain text\n\n".to_string())
        );
    }

    #[test]
    fn renders_binary_file_marker_without_plain_text() {
        let file = SlackMessageFile {
            name: Some("alert.eml".to_string()),
            title: Some("Alert email".to_string()),
            plain_text: None,
            is_binary: true,
        };

        assert_eq!(
            file.rendered_text(),
            Some("## Attached file title\n alert.eml\n\nThis is binary file".to_string())
        );
    }

    #[test]
    fn renders_multiple_files_as_paragraphs() {
        let rendered = render_slack_message_files_text(&[
            SlackMessageFile {
                name: Some("one.txt".to_string()),
                title: None,
                plain_text: Some("first".to_string()),
                is_binary: false,
            },
            SlackMessageFile {
                name: Some("two.txt".to_string()),
                title: None,
                plain_text: Some("second".to_string()),
                is_binary: false,
            },
        ]);

        assert_eq!(
            rendered,
            Some(
                "## Attached file title\n one.txt\n\n## Plain text\nfirst\n\n\n## Attached file title\n two.txt\n\n## Plain text\nsecond\n"
                    .to_string()
            )
        );
    }
}
