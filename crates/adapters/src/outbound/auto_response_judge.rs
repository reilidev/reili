use std::sync::Arc;

use async_trait::async_trait;
use reili_core::error::PortError;
use reili_core::messaging::slack::{
    AutoResponseJudgeDecision, AutoResponseJudgeInput, AutoResponseJudgePort,
};
use reili_core::secret::SecretString;
use rig::client::ProviderClient;
use rig::extractor::ExtractionError;
use rig::prelude::CompletionClient;
use rig::providers::{anthropic, openai};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::outbound::agents::{VertexAiGeminiClient, create_bedrock_client};

const JUDGE_MAX_TOKENS: u64 = 512;
const DEFAULT_JUDGE_POLICY: &str = "\
Respond when the Slack post signals a production issue worth investigating proactively: \
alerts, errors, failures, outages, latency degradation, or suspicious anomalies. \
Do not respond to casual conversation, announcements, scheduled-maintenance notices, \
or reports that the issue is already resolved.";

/// Single-shot judge built on the same rig completion clients as the task
/// runners; one implementation serves every provider.
pub struct AutoResponseJudgeAdapter<C>
where
    C: CompletionClient,
{
    client: C,
    model: String,
}

impl<C> AutoResponseJudgeAdapter<C>
where
    C: CompletionClient,
{
    pub fn new(client: C, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C> AutoResponseJudgePort for AutoResponseJudgeAdapter<C>
where
    C: CompletionClient + Send + Sync,
    C::CompletionModel: 'static,
{
    async fn judge(
        &self,
        input: AutoResponseJudgeInput,
    ) -> Result<AutoResponseJudgeDecision, PortError> {
        let result = self
            .client
            .extractor::<RawJudgeDecision>(self.model.clone())
            .preamble(&build_judge_preamble(&input))
            .max_tokens(JUDGE_MAX_TOKENS)
            .build()
            .extract(&build_judge_input_text(&input))
            .await;

        match result {
            Ok(decision) => {
                let reason = decision.reason.trim().to_string();
                Ok(AutoResponseJudgeDecision {
                    respond: decision.respond,
                    reason: if reason.is_empty() {
                        None
                    } else {
                        Some(reason)
                    },
                })
            }
            Err(ExtractionError::CompletionError(error)) => Err(PortError::new(format!(
                "Auto-response judge request failed: {error}"
            ))),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Auto-response judge could not extract a decision; treating as respond=false"
                );
                Ok(AutoResponseJudgeDecision {
                    respond: false,
                    reason: None,
                })
            }
        }
    }
}

pub fn create_openai_auto_response_judge_port(
    api_key: SecretString,
    model: String,
) -> Arc<dyn AutoResponseJudgePort> {
    Arc::new(AutoResponseJudgeAdapter::new(
        openai::Client::from_val(api_key.expose().to_string().into()),
        model,
    ))
}

pub fn create_anthropic_auto_response_judge_port(
    api_key: SecretString,
    model: String,
) -> Arc<dyn AutoResponseJudgePort> {
    Arc::new(AutoResponseJudgeAdapter::new(
        anthropic::Client::from_val(api_key.expose().to_string()),
        model,
    ))
}

pub struct CreateBedrockAutoResponseJudgePortInput {
    pub model_id: String,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
}

pub async fn create_bedrock_auto_response_judge_port(
    input: CreateBedrockAutoResponseJudgePortInput,
) -> Arc<dyn AutoResponseJudgePort> {
    let client =
        create_bedrock_client(input.aws_profile.as_deref(), input.aws_region.as_deref()).await;

    Arc::new(AutoResponseJudgeAdapter::new(client, input.model_id))
}

pub fn create_vertex_ai_auto_response_judge_port(
    client: VertexAiGeminiClient,
    model_id: String,
) -> Arc<dyn AutoResponseJudgePort> {
    Arc::new(AutoResponseJudgeAdapter::new(client, model_id))
}

fn build_judge_preamble(input: &AutoResponseJudgeInput) -> String {
    format!(
        "You are working as a software engineer on a team. For each Slack message, your only task is to decide whether or not to react to it.
Following the policy, determine whether you should react to the Slack message. Your output must strictly conform to the JSON schema.

Write the \"reason\" field in {language}.

## Policy
{policy}

",
        language = input.language,
        policy = input.policy.as_deref().unwrap_or(DEFAULT_JUDGE_POLICY),
    )
}

fn build_judge_input_text(input: &AutoResponseJudgeInput) -> String {
    let mut text = format!("## Use message\n{}\n\n", input.message_text);
    if !input.thread_context.is_empty() {
        text.push_str("## Recent thread context (optional)\n");
        for ctx in &input.thread_context {
            text.push_str(&format!(
                "[{}] {}: {}\n",
                format_slack_ts(&ctx.ts),
                ctx.user,
                ctx.text
            ));
        }
    }
    text
}

fn format_slack_ts(ts: &str) -> String {
    use chrono::DateTime;
    ts.split('.')
        .next()
        .and_then(|secs| secs.parse::<i64>().ok())
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ts.to_string())
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct RawJudgeDecision {
    respond: bool,
    reason: String,
}

#[cfg(test)]
mod tests {
    use reili_core::messaging::slack::{AutoResponseContextMessage, AutoResponseJudgeInput};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        AutoResponseJudgeAdapter, AutoResponseJudgePort, build_judge_input_text,
        build_judge_preamble,
    };

    fn sample_input() -> AutoResponseJudgeInput {
        AutoResponseJudgeInput {
            policy: Some("React to production incidents.".to_string()),
            message_text: "error rate is spiking".to_string(),
            thread_context: Vec::new(),
            language: "English".to_string(),
        }
    }

    #[test]
    fn builds_preamble_with_policy_and_language() {
        let preamble = build_judge_preamble(&sample_input());

        assert!(preamble.contains("## Policy\nReact to production incidents."));
        assert!(preamble.contains("English"));
    }

    #[test]
    fn builds_preamble_with_default_policy_when_policy_is_omitted() {
        let mut input = sample_input();
        input.policy = None;

        let preamble = build_judge_preamble(&input);

        assert!(preamble.contains(super::DEFAULT_JUDGE_POLICY));
    }

    #[test]
    fn builds_input_text_with_message() {
        let text = build_judge_input_text(&sample_input());

        assert!(text.contains("## Use message\nerror rate is spiking"));
        assert!(!text.contains("Recent thread context"));
    }

    #[test]
    fn builds_input_text_with_thread_context_when_present() {
        let mut input = sample_input();
        input.thread_context = vec![AutoResponseContextMessage {
            ts: "1710000000.000000".to_string(),
            user: "U002".to_string(),
            text: "deploy finished".to_string(),
        }];

        let text = build_judge_input_text(&input);

        assert!(text.contains(
            "## Recent thread context (optional)\n[2024-03-09T16:00:00Z] U002: deploy finished"
        ));
    }

    fn openai_function_call_body(arguments: serde_json::Value) -> serde_json::Value {
        json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1_710_000_000,
            "status": "completed",
            "error": null,
            "incomplete_details": null,
            "instructions": null,
            "max_output_tokens": null,
            "model": "judge-model",
            "usage": null,
            "output": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "submit",
                    "arguments": serde_json::to_string(&arguments).unwrap(),
                    "status": "completed"
                }
            ]
        })
    }

    fn openai_text_body(text: &str) -> serde_json::Value {
        json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1_710_000_000,
            "status": "completed",
            "error": null,
            "incomplete_details": null,
            "instructions": null,
            "max_output_tokens": null,
            "model": "judge-model",
            "usage": null,
            "output": [
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{ "type": "output_text", "text": text }]
                }
            ]
        })
    }

    fn openai_test_adapter(
        server: &MockServer,
    ) -> AutoResponseJudgeAdapter<rig::providers::openai::Client> {
        let client = rig::providers::openai::Client::builder()
            .api_key("test-key")
            .base_url(server.uri())
            .build()
            .expect("build openai test client");

        AutoResponseJudgeAdapter::new(client, "judge-model".to_string())
    }

    #[tokio::test]
    async fn judges_via_rig_extractor() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(openai_function_call_body(
                    json!({"respond": true, "reason": "incident signal"}),
                )),
            )
            .mount(&server)
            .await;

        let decision = openai_test_adapter(&server)
            .judge(sample_input())
            .await
            .expect("judge should succeed");

        assert!(decision.respond);
        assert_eq!(decision.reason.as_deref(), Some("incident signal"));
    }

    #[tokio::test]
    async fn falls_back_to_respond_false_when_model_does_not_call_submit_tool() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(openai_text_body("I cannot decide right now.")),
            )
            .mount(&server)
            .await;

        let decision = openai_test_adapter(&server)
            .judge(sample_input())
            .await
            .expect("judge should not error on extraction failure");

        assert!(!decision.respond);
        assert_eq!(decision.reason, None);
    }

    #[tokio::test]
    async fn returns_error_when_provider_request_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let error = openai_test_adapter(&server)
            .judge(sample_input())
            .await
            .expect_err("judge should fail");

        assert!(
            error.message.contains("Auto-response judge request failed"),
            "{}",
            error.message
        );
    }
}
