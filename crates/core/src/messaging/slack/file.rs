use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SlackMessageFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, alias = "plain_text", skip_serializing_if = "Option::is_none")]
    pub plain_text: Option<String>,
}

impl SlackMessageFile {
    pub fn rendered_text(&self) -> Option<String> {
        let name = self.name().unwrap_or_default();
        let plain_text = self.plain_text().unwrap_or_default();

        if name.is_empty() && plain_text.is_empty() {
            return None;
        }

        Some(format!("attached_file: {name}\nplain_text:\n{plain_text}"))
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
        };

        assert_eq!(
            file.rendered_text(),
            Some("attached_file: alert.eml\nplain_text:\nscheduled upgrade required".to_string())
        );
    }

    #[test]
    fn falls_back_to_title_when_name_is_missing() {
        let file = SlackMessageFile {
            name: None,
            title: Some("AWS Health Event".to_string()),
            plain_text: Some("important notice".to_string()),
        };

        assert_eq!(
            file.rendered_text(),
            Some("attached_file: AWS Health Event\nplain_text:\nimportant notice".to_string())
        );
    }

    #[test]
    fn renders_empty_plain_text_when_plain_text_is_missing() {
        let file = SlackMessageFile {
            name: Some("alert.eml".to_string()),
            title: Some("Alert email".to_string()),
            plain_text: None,
        };

        assert_eq!(
            file.rendered_text(),
            Some("attached_file: alert.eml\nplain_text:\n".to_string())
        );
    }

    #[test]
    fn renders_multiple_files_as_paragraphs() {
        let rendered = render_slack_message_files_text(&[
            SlackMessageFile {
                name: Some("one.txt".to_string()),
                title: None,
                plain_text: Some("first".to_string()),
            },
            SlackMessageFile {
                name: Some("two.txt".to_string()),
                title: None,
                plain_text: Some("second".to_string()),
            },
        ]);

        assert_eq!(
            rendered,
            Some(
                "attached_file: one.txt\nplain_text:\nfirst\n\nattached_file: two.txt\nplain_text:\nsecond"
                    .to_string()
            )
        );
    }
}
