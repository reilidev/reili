use chrono::{DateTime, Utc};
use reili_core::task::{TaskRequest, TaskRuntime};

use crate::outbound::agents::connector::ConnectorPromptFact;

pub struct BuildTaskPromptInput {
    pub request: TaskRequest,
    pub now: DateTime<Utc>,
    pub runtime: TaskRuntime,
    pub language: String,
    pub prompt_facts: Vec<ConnectorPromptFact>,
}

pub fn build_task_prompt(input: BuildTaskPromptInput) -> String {
    let trigger_message_user = &input.request.trigger_message.user;
    let trigger_message_text = input.request.trigger_message.rendered_text();
    let thread_transcript = build_thread_transcript(&input.request.thread_messages);
    let memory_context = build_memory_context(&input.request.memory_items);
    let current_context = build_current_context(&input);

    format!(
        "# Task Context
Output language: {language}
- Use {language} for all responses and reasoning.

Current context:
{current_context}

# Thread Context
{thread_transcript}

# Memory Context
{memory_context}

# User message
posted_by: {trigger_message_user}

{trigger_message_text}",
        language = input.language,
        current_context = current_context,
    )
}

fn build_current_context(input: &BuildTaskPromptInput) -> String {
    let mut lines = vec![
        format!("- Now: {}", input.now.to_rfc3339()),
        format!("- Slack Channel: {}", input.runtime.channel),
        format!("- Slack Thread: {}", input.runtime.thread_ts),
    ];
    lines.extend(
        input
            .prompt_facts
            .iter()
            .map(|fact| format!("- {}: {}", fact.label, fact.value)),
    );

    lines.join("\n")
}

fn build_thread_transcript(
    messages: &[reili_core::messaging::slack::SlackThreadMessage],
) -> String {
    messages
        .iter()
        .map(|message| {
            let text = message.rendered_text();
            let text = text.trim();
            format!(
                "ts: {}, iso_timestamp: {}, posted_by: {}\nmessage:{}",
                message.ts,
                message.iso_timestamp(),
                message.posted_by(),
                text
            )
        })
        .collect::<Vec<String>>()
        .join("\n---\n")
}

pub(super) fn build_memory_context(memory_items: &[reili_core::task::TaskMemoryItem]) -> String {
    if memory_items.is_empty() {
        return "No reusable memories found.".to_string();
    }

    memory_items
        .iter()
        .map(|item| {
            let source = item
                .source
                .permalink
                .as_deref()
                .unwrap_or("permalink_unavailable");
            format!(
                "source: {source}\nchannel: {}\nts: {}\nmemory:\n{}",
                item.source.channel_id,
                item.source.message_ts,
                item.content.trim()
            )
        })
        .collect::<Vec<String>>()
        .join("\n---\n")
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use reili_core::messaging::slack::{
        SlackMessage, SlackMessageFile, SlackThreadMessage, SlackTriggerType,
    };
    use reili_core::task::{TaskMemoryItem, TaskMemorySource, TaskRequest, TaskRuntime};

    use super::{BuildTaskPromptInput, build_task_prompt};
    use crate::outbound::agents::connector::ConnectorPromptFact;

    fn sample_trigger_message() -> SlackMessage {
        SlackMessage {
            slack_event_id: "Ev001".to_string(),
            team_id: Some("T001".to_string()),
            action_token: None,
            trigger: SlackTriggerType::AppMention,
            channel: "C001".to_string(),
            user: "U001".to_string(),
            actor_is_bot: false,
            text: "Please investigate this alert".to_string(),
            legacy_attachments: Vec::new(),
            files: Vec::new(),
            ts: "1710000000.000001".to_string(),
            thread_ts: None,
        }
    }

    fn sample_runtime() -> TaskRuntime {
        TaskRuntime {
            started_at_iso: "2026-01-01T00:00:00Z".to_string(),
            channel: "C001".to_string(),
            thread_ts: "1710000000.000001".to_string(),
            retry_count: 0,
        }
    }

    fn sample_prompt_facts() -> Vec<ConnectorPromptFact> {
        vec![
            ConnectorPromptFact {
                label: "GitHub Organization Scope".to_string(),
                value: "acme".to_string(),
            },
            ConnectorPromptFact {
                label: "Datadog Site".to_string(),
                value: "datadoghq.com".to_string(),
            },
        ]
    }

    fn build_sample_task_prompt(request: &TaskRequest) -> String {
        build_task_prompt(BuildTaskPromptInput {
            request: request.clone(),
            now: fixed_now(),
            runtime: sample_runtime(),
            language: "Japanese".to_string(),
            prompt_facts: sample_prompt_facts(),
        })
    }

    #[test]
    fn expands_trigger_and_thread_message_inputs() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![
                SlackThreadMessage {
                    ts: "1710000000.000001".to_string(),
                    user: Some("U123".to_string()),
                    text: "First message".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
                SlackThreadMessage {
                    ts: "1710000000.000002".to_string(),
                    user: Some("U456".to_string()),
                    text: " follow-up from bot ".to_string(),
                    legacy_attachments: Vec::new(),
                    files: Vec::new(),
                    metadata: None,
                },
            ],
            memory_items: Vec::new(),
        };
        let prompt = build_sample_task_prompt(&request);

        assert!(prompt.contains("Please investigate this alert"));
        assert!(prompt.contains("1710000000.000001"));
        assert!(prompt.contains("1710000000.000002"));
        assert!(prompt.contains("2024-03-09T16:00:00.000Z"));
        assert!(prompt.contains("U123"));
        assert!(prompt.contains("U456"));
        assert!(prompt.contains("First message"));
        assert!(prompt.contains("follow-up from bot"));
        assert!(prompt.contains("# User message\nposted_by: U001"));
    }

    #[test]
    fn includes_trigger_message_file_plain_text_in_prompt() {
        let mut trigger = sample_trigger_message();
        trigger.text = String::new();
        trigger.files = vec![SlackMessageFile {
            name: Some("aws-health.eml".to_string()),
            title: Some("AWS Health Event".to_string()),
            plain_text: Some("scheduled upgrade required".to_string()),
            is_binary: false,
            ..Default::default()
        }];
        let request = TaskRequest {
            trigger_message: trigger,
            thread_messages: vec![],
            memory_items: Vec::new(),
        };

        let prompt = build_sample_task_prompt(&request);

        assert!(prompt.contains("aws-health.eml"));
        assert!(prompt.contains("scheduled upgrade required"));
    }

    #[test]
    fn includes_thread_message_file_plain_text_in_prompt() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![SlackThreadMessage {
                ts: "1710000000.000002".to_string(),
                user: Some("U123".to_string()),
                text: String::new(),
                legacy_attachments: Vec::new(),
                files: vec![SlackMessageFile {
                    name: Some("aws-health.eml".to_string()),
                    title: Some("AWS Health Event".to_string()),
                    plain_text: Some("scheduled upgrade required".to_string()),
                    is_binary: false,
                    ..Default::default()
                }],
                metadata: None,
            }],
            memory_items: Vec::new(),
        };

        let prompt = build_sample_task_prompt(&request);

        assert!(prompt.contains("U123"));
        assert!(prompt.contains("aws-health.eml"));
        assert!(prompt.contains("scheduled upgrade required"));
    }

    #[test]
    fn includes_memory_context_in_prompt() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![],
            memory_items: vec![TaskMemoryItem {
                source: TaskMemorySource {
                    channel_id: "C001".to_string(),
                    message_ts: "1760000000.000001".to_string(),
                    thread_ts: Some("1760000000.000000".to_string()),
                    permalink: Some(
                        "https://example.slack.com/archives/C001/p1760000000000001".to_string(),
                    ),
                },
                content: "- service: checkout-api".to_string(),
            }],
        };

        let prompt = build_sample_task_prompt(&request);

        assert!(prompt.contains("https://example.slack.com/archives/C001/p1760000000000001"));
        assert!(prompt.contains("C001"));
        assert!(prompt.contains("1760000000.000001"));
        assert!(prompt.contains("- service: checkout-api"));
    }

    #[test]
    fn includes_runtime_context_in_prompt() {
        let request = TaskRequest {
            trigger_message: sample_trigger_message(),
            thread_messages: vec![],
            memory_items: Vec::new(),
        };

        let prompt = build_task_prompt(BuildTaskPromptInput {
            request,
            now: fixed_now(),
            runtime: sample_runtime(),
            language: "Japanese".to_string(),
            prompt_facts: vec![
                ConnectorPromptFact {
                    label: "GitHub Organization Scope".to_string(),
                    value: "acme".to_string(),
                },
                ConnectorPromptFact {
                    label: "Datadog Site".to_string(),
                    value: "datadoghq.com".to_string(),
                },
                ConnectorPromptFact {
                    label: "esa Team".to_string(),
                    value: "docs".to_string(),
                },
            ],
        });

        assert!(prompt.contains("Output language: Japanese"));
        assert!(prompt.contains("- Now: 2026-01-01T00:00:00+00:00"));
        assert!(prompt.contains("- Slack Channel: C001"));
        assert!(prompt.contains("- Slack Thread: 1710000000.000001"));
        assert!(prompt.contains("- GitHub Organization Scope: acme"));
        assert!(prompt.contains("- Datadog Site: datadoghq.com"));
        assert!(prompt.contains("- esa Team: docs"));
        assert!(prompt.contains("- esa Team: docs\n\n# Thread Context"));
        assert!(prompt.contains("# Memory Context\nNo reusable memories found.\n\n# User message"));
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }
}
