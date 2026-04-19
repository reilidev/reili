use std::fmt;

use reili_core::secret::SecretString;

#[derive(Clone, PartialEq, Eq)]
pub enum SlackConnectionMode {
    Http,
    SocketMode { app_token: SecretString },
}

impl fmt::Debug for SlackConnectionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http => write!(f, "Http"),
            Self::SocketMode { .. } => write!(f, "SocketMode {{ app_token: [REDACTED] }}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConfig {
    pub provider: LlmProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProviderConfig {
    OpenAi(OpenAiLlmConfig),
    Anthropic(AnthropicLlmConfig),
    Bedrock(BedrockLlmConfig),
    VertexAi(VertexAiLlmConfig),
}

impl LlmProviderConfig {
    #[must_use]
    pub fn provider_name(&self) -> &str {
        match self {
            Self::OpenAi(_) => "openai",
            Self::Anthropic(_) => "anthropic",
            Self::Bedrock(_) => "bedrock",
            Self::VertexAi(_) => "vertexai",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiLlmConfig {
    pub api_key: SecretString,
    pub model: String,
    pub reasoning_effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicLlmConfig {
    pub api_key: SecretString,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockLlmConfig {
    pub model_id: String,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiLlmConfig {
    pub project_id: String,
    pub location: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubConfig {
    pub url: String,
    pub app_id: String,
    pub private_key: SecretString,
    pub installation_id: u32,
    pub scope_org: String,
}

impl GitHubConfig {
    #[must_use]
    pub fn scope_org(&self) -> &str {
        &self.scope_org
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub slack_bot_token: SecretString,
    pub slack_signing_secret: Option<SecretString>,
    pub slack_connection_mode: SlackConnectionMode,
    pub port: u16,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub datadog_api_key: SecretString,
    pub datadog_app_key: SecretString,
    pub datadog_site: String,
    pub llm: LlmConfig,
    pub github: GitHubConfig,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, GitHubConfig, LlmConfig, LlmProviderConfig, OpenAiLlmConfig, SecretString,
        SlackConnectionMode,
    };

    #[test]
    fn app_config_debug_redacts_secrets() {
        let config = AppConfig {
            slack_bot_token: SecretString::new("xoxb-secret".to_string()),
            slack_signing_secret: Some(SecretString::new("slack-signing-secret".to_string())),
            slack_connection_mode: SlackConnectionMode::SocketMode {
                app_token: SecretString::new("xapp-secret".to_string()),
            },
            port: 3000,
            worker_concurrency: 2,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
            datadog_api_key: SecretString::new("dd-api".to_string()),
            datadog_app_key: SecretString::new("dd-app".to_string()),
            datadog_site: "datadoghq.com".to_string(),
            llm: LlmConfig {
                provider: LlmProviderConfig::OpenAi(OpenAiLlmConfig {
                    api_key: SecretString::new("openai-secret".to_string()),
                    model: "gpt-5.3-codex".to_string(),
                    reasoning_effort: "low".to_string(),
                }),
            },
            github: GitHubConfig {
                url: "https://api.githubcopilot.com/mcp/".to_string(),
                app_id: "12345".to_string(),
                private_key: SecretString::new("private-key".to_string()),
                installation_id: 99,
                scope_org: "example-org".to_string(),
            },
            language: "English".to_string(),
            additional_system_prompt: None,
        };

        let debug_output = format!("{config:?}");

        assert!(!debug_output.contains("xoxb-secret"));
        assert!(!debug_output.contains("slack-signing-secret"));
        assert!(!debug_output.contains("xapp-secret"));
        assert!(!debug_output.contains("dd-api"));
        assert!(!debug_output.contains("dd-app"));
        assert!(!debug_output.contains("openai-secret"));
        assert!(!debug_output.contains("private-key"));
        assert!(debug_output.contains("[REDACTED]"));
    }
}
