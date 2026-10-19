use reili_core::work::{RecordRef, text::truncate_chars};

const MAX_REPORT_CHARS: usize = 3_500;

/// A work item the cycle wrote to, under the key the agent gave it in its report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchedItem {
    pub key: String,
    pub record: RecordRef,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub created: Vec<TouchedItem>,
    pub updated: Vec<TouchedItem>,
    pub skipped: Vec<String>,
}

impl Activity {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty()
    }
}

/// The agent's Slack message with each `[[key]]` turned into a link to the item it wrote to.
/// `None` when the agent left the message out.
#[must_use]
pub fn compose_report(message: Option<&str>, activity: &Activity) -> Option<String> {
    let message = message
        .map(str::trim)
        .filter(|message| !message.is_empty())?;
    Some(truncate_chars(
        &link_items(message, activity),
        MAX_REPORT_CHARS,
    ))
}

fn link_items(message: &str, activity: &Activity) -> String {
    let mut text = String::new();
    let mut rest = message;
    while let Some(start) = rest.find("[[") {
        let Some(length) = rest[start + 2..].find("]]") else {
            break;
        };
        text.push_str(&rest[..start]);
        text.push_str(&link(rest[start + 2..start + 2 + length].trim(), activity));
        rest = &rest[start + 2 + length + 2..];
    }
    text.push_str(rest);
    text
}

fn link(reference: &str, activity: &Activity) -> String {
    activity
        .created
        .iter()
        .chain(&activity.updated)
        .find(|item| item.record.key.as_str() == reference || item.key.trim() == reference)
        .map_or_else(
            || reference.to_string(),
            |item| format!("<{}|{}>", item.record.url, item.record.key),
        )
}

#[cfg(test)]
mod tests {
    use reili_core::work::{RecordKey, RecordRef};

    use super::{Activity, TouchedItem, compose_report};

    fn touched(key: &str, number: u64) -> TouchedItem {
        TouchedItem {
            key: key.to_string(),
            record: RecordRef {
                key: RecordKey::new(format!("#{number}")),
                url: format!("https://github.com/acme/work/issues/{number}"),
            },
        }
    }

    #[test]
    fn links_the_items_the_agent_refers_to() {
        let activity = Activity {
            created: vec![touched("checkout:5xx-after-deploy", 12)],
            updated: vec![touched("checkout:pool-size", 3)],
            ..Activity::default()
        };

        let text = compose_report(
            Some("Opened [[checkout:5xx-after-deploy]]. Waiting on your call in [[#3]]; [[unknown]] stays as text."),
            &activity,
        )
        .expect("report");

        assert_eq!(
            text,
            "Opened <https://github.com/acme/work/issues/12|#12>. Waiting on your call in <https://github.com/acme/work/issues/3|#3>; unknown stays as text."
        );
    }

    #[test]
    fn posts_nothing_without_a_message() {
        let activity = Activity {
            created: vec![touched("checkout:5xx", 1)],
            ..Activity::default()
        };

        assert_eq!(compose_report(None, &activity), None);
        assert_eq!(compose_report(Some("  "), &activity), None);
    }
}
