use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackMarkdownTextChunk {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackPlanUpdateChunk {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SlackTaskUpdateStatus {
    Pending,
    InProgress,
    Complete,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SlackChunkSourceType {
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackChunkSource {
    #[serde(rename = "type")]
    pub source_type: SlackChunkSourceType,
    pub url: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackTaskUpdateChunk {
    pub id: String,
    pub title: String,
    pub status: SlackTaskUpdateStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<SlackChunkSource>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SlackAnyChunk {
    MarkdownText(SlackMarkdownTextChunk),
    PlanUpdate(SlackPlanUpdateChunk),
    TaskUpdate(SlackTaskUpdateChunk),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SlackStreamBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackStartStreamInput {
    pub channel: String,
    pub thread_ts: String,
    pub recipient_user_id: String,
    pub recipient_team_id: Option<String>,
    pub markdown_text: Option<String>,
    pub chunks: Option<Vec<SlackAnyChunk>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackAppendStreamInput {
    pub channel: String,
    pub stream_ts: String,
    pub markdown_text: Option<String>,
    pub chunks: Option<Vec<SlackAnyChunk>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackStopStreamInput {
    pub channel: String,
    pub stream_ts: String,
    pub markdown_text: Option<String>,
    pub chunks: Option<Vec<SlackAnyChunk>>,
    pub blocks: Option<Vec<SlackStreamBlock>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackStartStreamOutput {
    pub stream_ts: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        SlackAnyChunk, SlackChunkSource, SlackChunkSourceType, SlackTaskUpdateChunk,
        SlackTaskUpdateStatus,
    };

    #[test]
    fn serializes_and_deserializes_chunks() {
        let value = SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: "task-1".to_string(),
            title: "Query metrics".to_string(),
            status: SlackTaskUpdateStatus::InProgress,
            details: Some("running".to_string()),
            output: None,
            sources: Some(vec![SlackChunkSource {
                source_type: SlackChunkSourceType::Url,
                url: "https://example.com".to_string(),
                text: "example".to_string(),
            }]),
        });

        let json = serde_json::to_string(&value).expect("serialize slack chunk");
        let restored: SlackAnyChunk = serde_json::from_str(&json).expect("deserialize slack chunk");

        assert_eq!(restored, value);
    }

    #[test]
    fn omits_optional_task_update_fields_when_none() {
        let value = SlackAnyChunk::TaskUpdate(SlackTaskUpdateChunk {
            id: "task-1".to_string(),
            title: "Query metrics".to_string(),
            status: SlackTaskUpdateStatus::InProgress,
            details: None,
            output: None,
            sources: None,
        });

        let json_value = serde_json::to_value(&value).expect("serialize slack chunk");
        assert_eq!(
            json_value,
            json!({
                "type": "task_update",
                "id": "task-1",
                "title": "Query metrics",
                "status": "in_progress",
            })
        );
    }
}
