use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SlackLegacyAttachment {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pretext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<SlackLegacyAttachmentField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SlackLegacyAttachmentField {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<bool>,
}

impl SlackLegacyAttachment {
    pub fn rendered_text(&self) -> Option<String> {
        let title = self
            .title
            .clone()
            .or_else(|| self.pretext.clone())
            .unwrap_or_default();
        let author_name = self.author_name.clone().unwrap_or_default();
        let body = self
            .text
            .clone()
            .or_else(|| self.fallback.clone())
            .unwrap_or_default();

        if self.title_link.is_none()
            && title.is_empty()
            && author_name.is_empty()
            && body.is_empty()
        {
            return None;
        }

        Some(
            format!("<{:?}|{}|{}> {}", self.title_link, title, author_name, body)
                .trim()
                .to_string(),
        )
    }
}

pub fn render_slack_legacy_attachments_text(
    attachments: &[SlackLegacyAttachment],
) -> Option<String> {
    let parts: Vec<String> = attachments
        .iter()
        .filter_map(SlackLegacyAttachment::rendered_text)
        .collect();

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        SlackLegacyAttachment, SlackLegacyAttachmentField, render_slack_legacy_attachments_text,
    };

    #[test]
    fn renders_text_with_simple_template() {
        let attachment = SlackLegacyAttachment {
            pretext: Some("pretext".to_string()),
            author_name: Some("author".to_string()),
            title_link: Some("https://example.com".to_string()),
            text: Some("body".to_string()),
            ..Default::default()
        };

        assert_eq!(
            attachment.rendered_text(),
            Some(r#"<Some("https://example.com")|pretext|author> body"#.to_string())
        );
    }

    #[test]
    fn falls_back_to_fallback_text_when_body_is_missing() {
        let attachment = SlackLegacyAttachment {
            title: Some("Alert".to_string()),
            fallback: Some("fallback".to_string()),
            ..Default::default()
        };

        assert_eq!(
            attachment.rendered_text(),
            Some("<None|Alert|> fallback".to_string())
        );
    }

    #[test]
    fn renders_multiple_legacy_attachments_as_paragraphs() {
        let attachments = vec![
            SlackLegacyAttachment {
                text: Some("first".to_string()),
                ..Default::default()
            },
            SlackLegacyAttachment {
                text: Some("second".to_string()),
                ..Default::default()
            },
        ];

        assert_eq!(
            render_slack_legacy_attachments_text(&attachments),
            Some("<None||> first\n\n<None||> second".to_string())
        );
    }

    #[test]
    fn returns_none_when_all_simple_fields_are_empty() {
        let attachment = SlackLegacyAttachment {
            fields: vec![SlackLegacyAttachmentField {
                title: Some("Severity".to_string()),
                value: Some("critical".to_string()),
                short: Some(true),
            }],
            blocks: vec![Value::String("ignored".to_string())],
            ..Default::default()
        };

        assert_eq!(attachment.rendered_text(), None);
    }
}
