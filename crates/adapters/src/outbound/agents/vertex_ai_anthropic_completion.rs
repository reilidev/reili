//! Thin `rig` completion adapter for Anthropic Claude hosted on Vertex AI.
//!
//! This project needs Vertex AI to run Claude Sonnet in the task runner, but
//! the existing `rig-vertexai` crate targets Gemini models and does not
//! directly cover Claude on Vertex AI. This module therefore provides the
//! minimal completion client implementation that `rig` expects.
//!
//! Its responsibility is limited to sending task-runner Claude completions to
//! Vertex AI `rawPredict`. Web search requests are implemented separately in
//! `vertex_ai_web_search_adapter.rs`.

use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_api::model::HttpBody;
use rig::OneOrMany;
use rig::client::CompletionClient;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
};
use rig::message::{
    self, AssistantContent, Message, MessageError, Reasoning, Text, ToolCall, ToolFunction,
};
use rig::streaming::StreamingCompletionResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::outbound::vertex_ai::{
    ANTHROPIC_PUBLISHER, ANTHROPIC_VERTEX_VERSION, vertex_ai_base_url,
};

#[derive(Clone)]
pub struct VertexAiAnthropicClient {
    prediction_service: PredictionService,
    project_id: String,
    location: String,
}

pub struct VertexAiAnthropicClientInput {
    pub project_id: String,
    pub location: String,
}

impl VertexAiAnthropicClient {
    pub async fn new(input: VertexAiAnthropicClientInput) -> Result<Self, String> {
        let prediction_service = PredictionService::builder()
            .with_endpoint(vertex_ai_base_url(&input.location))
            .build()
            .await
            .map_err(|error| error.to_string())?;

        Ok(Self {
            prediction_service,
            project_id: input.project_id,
            location: input.location,
        })
    }
}

impl CompletionClient for VertexAiAnthropicClient {
    type CompletionModel = VertexAiAnthropicCompletionModel;
}

#[derive(Clone)]
pub struct VertexAiAnthropicCompletionModel {
    client: VertexAiAnthropicClient,
    model: String,
}

impl CompletionModel for VertexAiAnthropicCompletionModel {
    type Response = VertexAnthropicCompletionResponse;
    type StreamingResponse = ();
    type Client = VertexAiAnthropicClient;

    fn make(client: &Self::Client, model: impl Into<String>) -> Self {
        Self {
            client: client.clone(),
            model: model.into(),
        }
    }

    async fn completion(
        &self,
        mut request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let request_model = request.model.clone().unwrap_or_else(|| self.model.clone());

        if request.max_tokens.is_none() {
            request.max_tokens = Some(default_max_tokens(&request_model));
        }

        let request_body = VertexAnthropicCompletionRequest::try_from(request)?;
        let body = serde_json::to_vec(&request_body).map_err(|error| {
            CompletionError::ProviderError(format!(
                "Failed to serialize Vertex AI Claude request: {error}"
            ))
        })?;
        let http_body = HttpBody::new()
            .set_content_type("application/json")
            .set_data(body);
        let response_bytes = self
            .client
            .prediction_service
            .raw_predict()
            .set_endpoint(vertex_model_endpoint(
                &self.client.project_id,
                &self.client.location,
                &request_model,
            ))
            .set_http_body(http_body)
            .send()
            .await
            .map(|response| response.data.to_vec())
            .map_err(|error| CompletionError::ProviderError(error.to_string()))?;
        let response_text = String::from_utf8_lossy(&response_bytes).to_string();
        let response: VertexAnthropicCompletionResponse = serde_json::from_slice(&response_bytes)
            .map_err(|error| {
            CompletionError::ResponseError(format!(
                "Failed to deserialize Vertex AI Claude response: {error}; body={response_text}"
            ))
        })?;

        if response.kind != "message" {
            return Err(CompletionError::ResponseError(extract_error_message(
                &response_text,
            )));
        }

        response.try_into()
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        Err(CompletionError::ProviderError(
            "Streaming is not supported for Vertex AI Anthropic completion".to_string(),
        ))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct VertexAnthropicCompletionRequest {
    anthropic_version: String,
    messages: Vec<VertexAnthropicMessage>,
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<VertexAnthropicToolChoice>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tools: Vec<VertexAnthropicToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<VertexAnthropicOutputConfig>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    additional_params: Option<Value>,
}

impl TryFrom<CompletionRequest> for VertexAnthropicCompletionRequest {
    type Error = CompletionError;

    fn try_from(request: CompletionRequest) -> Result<Self, Self::Error> {
        let max_tokens = request.max_tokens.ok_or_else(|| {
            CompletionError::RequestError("`max_tokens` must be set for Vertex AI Claude".into())
        })?;

        let mut full_history = Vec::new();
        if let Some(documents) = request.normalized_documents() {
            full_history.push(documents);
        }
        full_history.extend(request.chat_history);

        let messages = full_history
            .into_iter()
            .map(VertexAnthropicMessage::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let tools = request
            .tools
            .into_iter()
            .map(|tool| VertexAnthropicToolDefinition {
                name: tool.name,
                description: Some(tool.description),
                input_schema: tool.parameters,
            })
            .collect::<Vec<_>>();

        let output_config = request.output_schema.map(|schema| {
            let mut schema_value = schema.to_value();
            sanitize_schema(&mut schema_value);
            VertexAnthropicOutputConfig {
                format: VertexAnthropicOutputFormat::JsonSchema {
                    schema: schema_value,
                },
            }
        });

        Ok(Self {
            anthropic_version: ANTHROPIC_VERTEX_VERSION.to_string(),
            messages,
            max_tokens,
            system: request.preamble.filter(|value| !value.is_empty()),
            temperature: request.temperature,
            tool_choice: request
                .tool_choice
                .map(VertexAnthropicToolChoice::try_from)
                .transpose()?,
            tools,
            output_config,
            additional_params: request.additional_params,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct VertexAnthropicToolDefinition {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: Value,
}

#[derive(Default, Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VertexAnthropicToolChoice {
    #[default]
    Auto,
    None,
    Any,
    Tool {
        name: String,
    },
}

impl TryFrom<message::ToolChoice> for VertexAnthropicToolChoice {
    type Error = CompletionError;

    fn try_from(value: message::ToolChoice) -> Result<Self, Self::Error> {
        match value {
            message::ToolChoice::Auto => Ok(Self::Auto),
            message::ToolChoice::None => Ok(Self::None),
            message::ToolChoice::Required => Ok(Self::Any),
            message::ToolChoice::Specific { function_names } => {
                if function_names.len() != 1 {
                    return Err(CompletionError::ProviderError(
                        "Vertex AI Claude supports at most one explicitly selected tool"
                            .to_string(),
                    ));
                }

                Ok(Self::Tool {
                    name: function_names[0].clone(),
                })
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct VertexAnthropicMessage {
    role: VertexAnthropicRole,
    content: Vec<VertexAnthropicContent>,
}

impl TryFrom<Message> for VertexAnthropicMessage {
    type Error = MessageError;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        match message {
            Message::User { content } => Ok(Self {
                role: VertexAnthropicRole::User,
                content: content
                    .into_iter()
                    .map(VertexAnthropicContent::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            Message::Assistant { content, .. } => {
                let content =
                    content
                        .into_iter()
                        .try_fold(Vec::new(), |mut accumulated, item| {
                            accumulated.extend(vertex_content_from_assistant_content(item)?);
                            Ok::<Vec<VertexAnthropicContent>, MessageError>(accumulated)
                        })?;

                Ok(Self {
                    role: VertexAnthropicRole::Assistant,
                    content,
                })
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum VertexAnthropicRole {
    User,
    Assistant,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VertexAnthropicContent {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<VertexAnthropicToolResultContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
}

impl TryFrom<message::UserContent> for VertexAnthropicContent {
    type Error = MessageError;

    fn try_from(content: message::UserContent) -> Result<Self, Self::Error> {
        match content {
            message::UserContent::Text(Text { text }) => Ok(Self::Text { text }),
            message::UserContent::ToolResult(message::ToolResult { id, content, .. }) => {
                Ok(Self::ToolResult {
                    tool_use_id: id,
                    content: content
                        .into_iter()
                        .map(VertexAnthropicToolResultContent::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                    is_error: None,
                })
            }
            other => Err(MessageError::ConversionError(format!(
                "Unsupported Vertex AI Claude user content: {other:?}"
            ))),
        }
    }
}

fn vertex_content_from_assistant_content(
    content: AssistantContent,
) -> Result<Vec<VertexAnthropicContent>, MessageError> {
    match content {
        AssistantContent::Text(Text { text }) => Ok(vec![VertexAnthropicContent::Text { text }]),
        AssistantContent::ToolCall(ToolCall { id, function, .. }) => {
            Ok(vec![VertexAnthropicContent::ToolUse {
                id,
                name: function.name,
                input: function.arguments,
            }])
        }
        AssistantContent::Reasoning(reasoning) => {
            let mut converted = Vec::new();

            for item in reasoning.content {
                match item {
                    message::ReasoningContent::Text { text, signature } => {
                        converted.push(VertexAnthropicContent::Thinking {
                            thinking: text,
                            signature,
                        });
                    }
                    message::ReasoningContent::Summary(summary) => {
                        converted.push(VertexAnthropicContent::Thinking {
                            thinking: summary,
                            signature: None,
                        });
                    }
                    message::ReasoningContent::Redacted { data }
                    | message::ReasoningContent::Encrypted(data) => {
                        converted.push(VertexAnthropicContent::RedactedThinking { data });
                    }
                    _ => {
                        return Err(MessageError::ConversionError(
                            "Unsupported Vertex AI Claude reasoning block".to_string(),
                        ));
                    }
                }
            }

            if converted.is_empty() {
                return Err(MessageError::ConversionError(
                    "Cannot convert empty reasoning content for Vertex AI Claude".to_string(),
                ));
            }

            Ok(converted)
        }
        other => Err(MessageError::ConversionError(format!(
            "Unsupported Vertex AI Claude assistant content: {other:?}"
        ))),
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VertexAnthropicToolResultContent {
    Text { text: String },
}

impl TryFrom<message::ToolResultContent> for VertexAnthropicToolResultContent {
    type Error = MessageError;

    fn try_from(content: message::ToolResultContent) -> Result<Self, Self::Error> {
        match content {
            message::ToolResultContent::Text(Text { text }) => Ok(Self::Text { text }),
            other => Err(MessageError::ConversionError(format!(
                "Unsupported Vertex AI Claude tool result content: {other:?}"
            ))),
        }
    }
}

impl TryFrom<VertexAnthropicContent> for AssistantContent {
    type Error = MessageError;

    fn try_from(content: VertexAnthropicContent) -> Result<Self, Self::Error> {
        match content {
            VertexAnthropicContent::Text { text } => Ok(AssistantContent::Text(Text { text })),
            VertexAnthropicContent::ToolUse { id, name, input } => Ok(AssistantContent::ToolCall(
                ToolCall::new(id, ToolFunction::new(name, input)),
            )),
            VertexAnthropicContent::Thinking {
                thinking,
                signature,
            } => Ok(AssistantContent::Reasoning(Reasoning::new_with_signature(
                &thinking, signature,
            ))),
            VertexAnthropicContent::RedactedThinking { data } => {
                Ok(AssistantContent::Reasoning(Reasoning::redacted(data)))
            }
            VertexAnthropicContent::ToolResult { .. } => Err(MessageError::ConversionError(
                "Unexpected tool result in Vertex AI Claude response".to_string(),
            )),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VertexAnthropicCompletionResponse {
    #[serde(rename = "type")]
    kind: String,
    content: Vec<VertexAnthropicContent>,
    id: String,
    model: String,
    role: String,
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
    usage: VertexAnthropicUsage,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct VertexAnthropicUsage {
    input_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_input_tokens: Option<u64>,
    output_tokens: u64,
}

impl TryFrom<VertexAnthropicCompletionResponse>
    for CompletionResponse<VertexAnthropicCompletionResponse>
{
    type Error = CompletionError;

    fn try_from(response: VertexAnthropicCompletionResponse) -> Result<Self, Self::Error> {
        let choice = OneOrMany::many(
            response
                .content
                .iter()
                .cloned()
                .map(AssistantContent::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| {
            CompletionError::ResponseError(
                "Vertex AI Claude response contained no assistant content".to_string(),
            )
        })?;

        let usage = Usage {
            input_tokens: response.usage.input_tokens
                + response.usage.cache_creation_input_tokens.unwrap_or(0),
            output_tokens: response.usage.output_tokens,
            total_tokens: response.usage.input_tokens
                + response.usage.cache_creation_input_tokens.unwrap_or(0)
                + response.usage.output_tokens,
            cached_input_tokens: response.usage.cache_read_input_tokens.unwrap_or(0),
        };

        let message_id = Some(response.id.clone());

        Ok(CompletionResponse {
            choice,
            usage,
            raw_response: response,
            message_id,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct VertexAnthropicOutputConfig {
    format: VertexAnthropicOutputFormat,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VertexAnthropicOutputFormat {
    JsonSchema { schema: Value },
}

fn sanitize_schema(schema: &mut Value) {
    if let Value::Object(object) = schema {
        let is_object_schema = object.get("type") == Some(&Value::String("object".to_string()))
            || object.contains_key("properties");

        if is_object_schema && !object.contains_key("additionalProperties") {
            object.insert("additionalProperties".to_string(), Value::Bool(false));
        }

        if let Some(Value::Object(properties)) = object.get("properties") {
            let required = properties.keys().cloned().map(Value::String).collect();
            object.insert("required".to_string(), Value::Array(required));
        }

        if let Some(defs) = object.get_mut("$defs")
            && let Value::Object(definitions) = defs
        {
            for value in definitions.values_mut() {
                sanitize_schema(value);
            }
        }

        if let Some(properties) = object.get_mut("properties")
            && let Value::Object(map) = properties
        {
            for value in map.values_mut() {
                sanitize_schema(value);
            }
        }

        if let Some(items) = object.get_mut("items") {
            sanitize_schema(items);
        }

        for key in ["anyOf", "oneOf", "allOf"] {
            if let Some(variants) = object.get_mut(key)
                && let Value::Array(values) = variants
            {
                for value in values.iter_mut() {
                    sanitize_schema(value);
                }
            }
        }
    }
}

fn vertex_model_endpoint(project_id: &str, location: &str, model: &str) -> String {
    format!(
        "projects/{project_id}/locations/{location}/publishers/{ANTHROPIC_PUBLISHER}/models/{model}"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeModelFamily {
    Opus,
    Sonnet,
    Haiku,
}

impl ClaudeModelFamily {
    fn parse(model: &str) -> Option<Self> {
        let name = model.split('@').next().unwrap_or(model);
        if name.starts_with("claude-opus-4") {
            Some(Self::Opus)
        } else if name.starts_with("claude-sonnet-4") {
            Some(Self::Sonnet)
        } else if name.starts_with("claude-haiku-4") {
            Some(Self::Haiku)
        } else {
            None
        }
    }

    fn default_max_tokens(self) -> u64 {
        match self {
            Self::Opus => 32_000,
            Self::Sonnet => 64_000,
            Self::Haiku => 8_192,
        }
    }
}

fn default_max_tokens(model: &str) -> u64 {
    ClaudeModelFamily::parse(model)
        .map(ClaudeModelFamily::default_max_tokens)
        .unwrap_or(8_192)
}

fn extract_error_message(response_text: &str) -> String {
    serde_json::from_str::<Value>(response_text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message").or_else(|| error.get("details")))
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
        })
        .unwrap_or_else(|| response_text.to_string())
}

#[cfg(test)]
mod tests {
    use rig::completion::{CompletionRequest, ToolDefinition};
    use rig::message::{Message, ToolChoice, UserContent};
    use serde_json::json;

    use super::{
        ANTHROPIC_VERTEX_VERSION, AssistantContent, CompletionResponse, OneOrMany, Reasoning, Text,
        VertexAnthropicCompletionRequest, VertexAnthropicCompletionResponse,
        VertexAnthropicContent, VertexAnthropicToolChoice, VertexAnthropicUsage,
        default_max_tokens, vertex_ai_base_url, vertex_model_endpoint,
    };

    fn sample_request() -> CompletionRequest {
        CompletionRequest {
            model: None,
            preamble: Some("system prompt".to_string()),
            chat_history: OneOrMany::one(Message::User {
                content: OneOrMany::one(UserContent::Text(Text {
                    text: "hello".to_string(),
                })),
            }),
            documents: vec![],
            tools: vec![ToolDefinition {
                name: "search_web".to_string(),
                description: "Searches the web".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }),
            }],
            temperature: Some(0.2),
            max_tokens: Some(512),
            tool_choice: Some(ToolChoice::Required),
            additional_params: Some(json!({ "top_p": 0.9 })),
            output_schema: None,
        }
    }

    #[test]
    fn builds_vertex_request_from_completion_request() {
        let request = VertexAnthropicCompletionRequest::try_from(sample_request())
            .expect("build vertex request");

        assert_eq!(request.anthropic_version, ANTHROPIC_VERTEX_VERSION);
        assert_eq!(request.max_tokens, 512);
        assert_eq!(request.system.as_deref(), Some("system prompt"));
        assert_eq!(request.tool_choice, Some(VertexAnthropicToolChoice::Any));
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.additional_params, Some(json!({ "top_p": 0.9 })));
    }

    #[test]
    fn vertex_completion_response_converts_to_rig_response() {
        let response = VertexAnthropicCompletionResponse {
            kind: "message".to_string(),
            content: vec![
                VertexAnthropicContent::Thinking {
                    thinking: "inspect logs".to_string(),
                    signature: Some("sig-1".to_string()),
                },
                VertexAnthropicContent::Text {
                    text: "investigation complete".to_string(),
                },
            ],
            id: "msg-1".to_string(),
            model: "claude-sonnet".to_string(),
            role: "assistant".to_string(),
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            usage: VertexAnthropicUsage {
                input_tokens: 100,
                cache_read_input_tokens: Some(5),
                cache_creation_input_tokens: None,
                output_tokens: 20,
            },
        };

        let converted: CompletionResponse<VertexAnthropicCompletionResponse> =
            response.try_into().expect("convert response");

        assert_eq!(converted.message_id.as_deref(), Some("msg-1"));
        assert_eq!(converted.usage.input_tokens, 100);
        assert_eq!(converted.usage.output_tokens, 20);
        assert_eq!(converted.usage.cached_input_tokens, 5);

        let items = converted.choice.into_iter().collect::<Vec<_>>();
        assert!(matches!(
            items.first(),
            Some(AssistantContent::Reasoning(Reasoning { .. }))
        ));
        assert!(matches!(
            items.get(1),
            Some(AssistantContent::Text(Text { text })) if text == "investigation complete"
        ));
    }

    #[test]
    fn builds_expected_vertex_urls() {
        assert_eq!(
            vertex_ai_base_url("global"),
            "https://aiplatform.googleapis.com"
        );
        assert_eq!(
            vertex_ai_base_url("us-east5"),
            "https://us-east5-aiplatform.googleapis.com"
        );
        assert_eq!(
            vertex_model_endpoint("proj", "us-east5", "claude-sonnet-4"),
            "projects/proj/locations/us-east5/publishers/anthropic/models/claude-sonnet-4"
        );
    }

    #[test]
    fn picks_reasonable_default_max_tokens() {
        assert_eq!(default_max_tokens("claude-opus-4-5@20251101"), 32_000);
        assert_eq!(default_max_tokens("claude-opus-4-6@20260301"), 32_000);
        assert_eq!(default_max_tokens("claude-sonnet-4-5@20250929"), 64_000);
        assert_eq!(default_max_tokens("claude-sonnet-4-6@20260301"), 64_000);
        assert_eq!(default_max_tokens("claude-haiku-4-5@20251001"), 8_192);
        assert_eq!(default_max_tokens("unknown-model"), 8_192);
    }

    #[test]
    fn parses_claude_model_family() {
        use super::ClaudeModelFamily;

        assert_eq!(
            ClaudeModelFamily::parse("claude-opus-4-6@20260301"),
            Some(ClaudeModelFamily::Opus)
        );
        assert_eq!(
            ClaudeModelFamily::parse("claude-sonnet-4-5@20250929"),
            Some(ClaudeModelFamily::Sonnet)
        );
        assert_eq!(
            ClaudeModelFamily::parse("claude-haiku-4-5@20251001"),
            Some(ClaudeModelFamily::Haiku)
        );
        assert_eq!(ClaudeModelFamily::parse("gpt-4o"), None);
    }
}
