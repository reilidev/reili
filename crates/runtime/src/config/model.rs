use std::fmt;

use reili_core::messaging::slack::SlackChannelNamePattern;
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
pub struct SlackAuthorizationConfig {
    pub actors: Option<SlackAuthorizationActors>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAuthorizationActors {
    pub user_ids: Option<Vec<String>>,
    pub user_group_ids: Option<Vec<String>>,
    pub allow_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackChannelConfig {
    pub names: Vec<SlackChannelNamePattern>,
    pub mention: bool,
    pub auto_response: bool,
    /// Judge policy override for this channel; the judge's built-in policy
    /// is used when omitted.
    pub auto_response_policy: Option<String>,
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

/// Provider configuration for the single-shot auto-response judge. The judge has
/// no sub-agent, so this carries only the fields the judge call actually needs
/// (unlike [`LlmProviderConfig`], which also tracks a sub-agent model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeProviderConfig {
    OpenAi {
        api_key: SecretString,
        model: String,
    },
    Anthropic {
        api_key: SecretString,
        model: String,
    },
    Bedrock {
        model_id: String,
        aws_profile: Option<String>,
        aws_region: Option<String>,
    },
    VertexAi {
        project_id: String,
        location: String,
        model_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiLlmConfig {
    pub api_key: SecretString,
    pub model: String,
    pub sub_agent_model: String,
    pub reasoning_effort: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicLlmConfig {
    pub api_key: SecretString,
    pub model: String,
    pub sub_agent_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockLlmConfig {
    pub model_id: String,
    pub sub_agent_model_id: String,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiLlmConfig {
    pub project_id: String,
    pub location: String,
    pub model_id: String,
    pub sub_agent_model_id: String,
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
pub struct EsaConfig {
    pub team_name: String,
    pub access_token: SecretString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraConfig {
    pub site: String,
    pub service_account_api_token: SecretString,
}

/// Channel memory backed by a pre-created shared Slack Canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackCanvasMemoryConfig {
    pub canvas_id: String,
    pub cap: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub slack_bot_token: SecretString,
    pub slack_signing_secret: Option<SecretString>,
    pub slack_connection_mode: SlackConnectionMode,
    pub slack_authorization: Option<SlackAuthorizationConfig>,
    pub slack_channels: Vec<SlackChannelConfig>,
    pub port: u16,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub datadog_api_key: SecretString,
    pub datadog_app_key: SecretString,
    pub datadog_site: String,
    pub llm: LlmConfig,
    /// Provider used by the auto-response judge; resolved only when at least
    /// one channel enables `auto_response`.
    pub judge_llm: Option<JudgeProviderConfig>,
    pub github: GitHubConfig,
    pub esa: Option<EsaConfig>,
    pub jira: Option<JiraConfig>,
    /// Channel memory backend; `None` disables the memory feature entirely.
    pub memory: Option<SlackCanvasMemoryConfig>,
    pub language: String,
    pub additional_system_prompt: Option<String>,
}

impl AppConfig {
    /// Channel name patterns for mention authorization, aggregated from
    /// `mention = true` entries. An empty list denies mentions in all channels.
    pub fn mention_channel_patterns(&self) -> Vec<SlackChannelNamePattern> {
        self.slack_channels
            .iter()
            .filter(|channel| channel.mention)
            .flat_map(|channel| channel.names.iter().cloned())
            .collect()
    }

    /// Channels eligible for auto-response judgement.
    pub fn auto_response_channels(&self) -> impl Iterator<Item = &SlackChannelConfig> {
        self.slack_channels
            .iter()
            .filter(|channel| channel.auto_response)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, EsaConfig, GitHubConfig, JiraConfig, JudgeProviderConfig, LlmConfig,
        LlmProviderConfig, OpenAiLlmConfig, SecretString, SlackChannelConfig,
        SlackChannelNamePattern, SlackConnectionMode,
    };

    #[test]
    fn app_config_debug_redacts_secrets() {
        let config = AppConfig {
            slack_bot_token: SecretString::new("xoxb-secret".to_string()),
            slack_signing_secret: Some(SecretString::new("slack-signing-secret".to_string())),
            slack_connection_mode: SlackConnectionMode::SocketMode {
                app_token: SecretString::new("xapp-secret".to_string()),
            },
            slack_authorization: None,
            slack_channels: Vec::new(),
            port: 3000,
            worker_concurrency: 8,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
            datadog_api_key: SecretString::new("dd-api".to_string()),
            datadog_app_key: SecretString::new("dd-app".to_string()),
            datadog_site: "datadoghq.com".to_string(),
            llm: LlmConfig {
                provider: LlmProviderConfig::OpenAi(OpenAiLlmConfig {
                    api_key: SecretString::new("openai-secret".to_string()),
                    model: "gpt-5.3-codex".to_string(),
                    sub_agent_model: "gpt-5.3-codex".to_string(),
                    reasoning_effort: "low".to_string(),
                }),
            },
            judge_llm: Some(JudgeProviderConfig::OpenAi {
                api_key: SecretString::new("judge-openai-secret".to_string()),
                model: "gpt-5.4-mini".to_string(),
            }),
            github: GitHubConfig {
                url: "https://api.githubcopilot.com/mcp/".to_string(),
                app_id: "12345".to_string(),
                private_key: SecretString::new("private-key".to_string()),
                installation_id: 99,
                scope_org: "example-org".to_string(),
            },
            esa: Some(EsaConfig {
                team_name: "docs".to_string(),
                access_token: SecretString::new("esa-token".to_string()),
            }),
            jira: Some(JiraConfig {
                site: "acme.atlassian.net".to_string(),
                service_account_api_token: SecretString::new(
                    "jira-service-account-token".to_string(),
                ),
            }),
            memory: None,
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
        assert!(!debug_output.contains("judge-openai-secret"));
        assert!(!debug_output.contains("private-key"));
        assert!(!debug_output.contains("esa-token"));
        assert!(!debug_output.contains("jira-service-account-token"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    fn channel(
        names: Vec<&str>,
        mention: bool,
        auto_response: bool,
        auto_response_policy: Option<&str>,
    ) -> SlackChannelConfig {
        SlackChannelConfig {
            names: names
                .into_iter()
                .map(|name| SlackChannelNamePattern::new(name.to_string()))
                .collect(),
            mention,
            auto_response,
            auto_response_policy: auto_response_policy.map(ToString::to_string),
        }
    }

    fn config_with_channels(slack_channels: Vec<SlackChannelConfig>) -> AppConfig {
        AppConfig {
            slack_bot_token: SecretString::new("xoxb-secret".to_string()),
            slack_signing_secret: None,
            slack_connection_mode: SlackConnectionMode::Http,
            slack_authorization: None,
            slack_channels,
            port: 3000,
            worker_concurrency: 8,
            job_max_retry: 2,
            job_backoff_ms: 1_000,
            datadog_api_key: SecretString::new("dd-api".to_string()),
            datadog_app_key: SecretString::new("dd-app".to_string()),
            datadog_site: "datadoghq.com".to_string(),
            llm: LlmConfig {
                provider: LlmProviderConfig::OpenAi(OpenAiLlmConfig {
                    api_key: SecretString::new("openai-secret".to_string()),
                    model: "gpt-5.3-codex".to_string(),
                    sub_agent_model: "gpt-5.3-codex".to_string(),
                    reasoning_effort: "low".to_string(),
                }),
            },
            judge_llm: None,
            github: GitHubConfig {
                url: "https://api.githubcopilot.com/mcp/".to_string(),
                app_id: "12345".to_string(),
                private_key: SecretString::new("private-key".to_string()),
                installation_id: 99,
                scope_org: "example-org".to_string(),
            },
            esa: None,
            jira: None,
            memory: None,
            language: "English".to_string(),
            additional_system_prompt: None,
        }
    }

    #[test]
    fn aggregates_mention_channel_patterns_from_mention_entries_only() {
        let config = config_with_channels(vec![
            channel(vec!["incidents", "alerts-*"], true, true, Some("prompt")),
            channel(vec!["aws-health"], false, true, Some("prompt")),
            channel(vec!["team-sre"], true, false, None),
        ]);

        let patterns = config
            .mention_channel_patterns()
            .iter()
            .map(|pattern| pattern.as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(patterns, vec!["incidents", "alerts-*", "team-sre"]);
    }

    #[test]
    fn returns_empty_mention_patterns_for_empty_channels_table() {
        let config = config_with_channels(Vec::new());

        assert!(config.mention_channel_patterns().is_empty());
    }

    #[test]
    fn filters_auto_response_channels() {
        let config = config_with_channels(vec![
            channel(vec!["incidents"], true, true, Some("prompt")),
            channel(vec!["team-sre"], true, false, None),
        ]);

        let auto_response_names = config
            .auto_response_channels()
            .flat_map(|channel| channel.names.iter().map(|name| name.as_str().to_string()))
            .collect::<Vec<_>>();

        assert_eq!(auto_response_names, vec!["incidents"]);
    }
}
