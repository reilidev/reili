use std::fmt;

use thiserror::Error;

const DEFAULT_APP_PORT: u16 = 3000;
const DEFAULT_WORKER_CONCURRENCY: u32 = 2;
const DEFAULT_DATADOG_SITE: &str = "datadoghq.com";
const DEFAULT_GITHUB_MCP_URL: &str = "https://api.githubcopilot.com/mcp/";
const DEFAULT_LANGUAGE: &str = "English";
const DEFAULT_JOB_MAX_RETRY: u32 = 2;
const DEFAULT_JOB_BACKOFF_MS: u64 = 1_000;
const DEFAULT_OPENAI_TASK_RUNNER_MODEL: &str = "gpt-5.3-codex";
const SUPPORTED_ANTHROPIC_MODELS: &[&str] =
    &["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"];

#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

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
pub struct SlackAuthConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskConfig {
    pub datadog_api_key: String,
    pub datadog_app_key: String,
    pub datadog_site: String,
    pub llm: LlmConfig,
    pub language: String,
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
    pub api_key: String,
    pub task_runner_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicLlmConfig {
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockLlmConfig {
    pub model_id: String,
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
    pub slack_bot_token: String,
    pub slack_signing_secret: Option<String>,
    pub slack_connection_mode: SlackConnectionMode,
    pub port: u16,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub datadog_api_key: String,
    pub datadog_app_key: String,
    pub datadog_site: String,
    pub llm: LlmConfig,
    pub github: GitHubConfig,
    pub language: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvConfigError {
    #[error("Missing required environment variable: {name}")]
    MissingRequired { name: String },
    #[error("Invalid {name} value: {value}")]
    InvalidValue { name: String, value: String },
}

pub fn load_app_config() -> Result<AppConfig, EnvConfigError> {
    load_app_config_with_env(&ProcessEnvironment)
}

#[cfg_attr(test, mockall::automock)]
trait EnvironmentReader {
    fn get(&self, name: &str) -> Option<String>;
}

struct ProcessEnvironment;

impl EnvironmentReader for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

fn load_app_config_with_env(env: &dyn EnvironmentReader) -> Result<AppConfig, EnvConfigError> {
    let connection_mode = read_slack_connection_mode(env)?;
    let slack_auth = read_slack_auth_config(env, &connection_mode)?;
    let task_config = read_task_config(env)?;

    Ok(AppConfig {
        slack_bot_token: slack_auth.slack_bot_token,
        slack_signing_secret: slack_auth.slack_signing_secret,
        slack_connection_mode: connection_mode,
        port: read_port(env, "PORT", DEFAULT_APP_PORT)?,
        worker_concurrency: DEFAULT_WORKER_CONCURRENCY,
        job_max_retry: DEFAULT_JOB_MAX_RETRY,
        job_backoff_ms: DEFAULT_JOB_BACKOFF_MS,
        datadog_api_key: task_config.datadog_api_key,
        datadog_app_key: task_config.datadog_app_key,
        datadog_site: task_config.datadog_site,
        llm: task_config.llm,
        github: read_github_config(env)?,
        language: task_config.language,
    })
}

fn read_slack_connection_mode(
    env: &dyn EnvironmentReader,
) -> Result<SlackConnectionMode, EnvConfigError> {
    let socket_mode = read_or_default(env, "SLACK_SOCKET_MODE", "true");
    if socket_mode == "true" {
        let app_token = read_required_env(env, "SLACK_APP_TOKEN")?;
        if !app_token.starts_with("xapp-") {
            return Err(EnvConfigError::InvalidValue {
                name: "SLACK_APP_TOKEN".to_string(),
                value: "must start with xapp-".to_string(),
            });
        }
        Ok(SlackConnectionMode::SocketMode {
            app_token: SecretString::new(app_token),
        })
    } else {
        Ok(SlackConnectionMode::Http)
    }
}

fn read_slack_auth_config(
    env: &dyn EnvironmentReader,
    connection_mode: &SlackConnectionMode,
) -> Result<SlackAuthConfig, EnvConfigError> {
    let slack_bot_token = read_required_env(env, "SLACK_BOT_TOKEN")?;
    let slack_signing_secret = match connection_mode {
        SlackConnectionMode::Http => Some(read_required_env(env, "SLACK_SIGNING_SECRET")?),
        SlackConnectionMode::SocketMode { .. } => {
            read_optional_non_empty_env(env, "SLACK_SIGNING_SECRET")
        }
    };
    Ok(SlackAuthConfig {
        slack_bot_token,
        slack_signing_secret,
    })
}

fn read_task_config(env: &dyn EnvironmentReader) -> Result<TaskConfig, EnvConfigError> {
    Ok(TaskConfig {
        datadog_api_key: read_required_env(env, "DATADOG_API_KEY")?,
        datadog_app_key: read_required_env(env, "DATADOG_APP_KEY")?,
        datadog_site: read_or_default(env, "DATADOG_SITE", DEFAULT_DATADOG_SITE),
        llm: read_llm_config(env)?,
        language: read_or_default(env, "LANGUAGE", DEFAULT_LANGUAGE),
    })
}

fn read_llm_config(env: &dyn EnvironmentReader) -> Result<LlmConfig, EnvConfigError> {
    let provider = read_required_env(env, "LLM_PROVIDER")?;
    let provider_config = match provider.as_str() {
        "openai" => LlmProviderConfig::OpenAi(read_openai_llm_config(env)?),
        "anthropic" => LlmProviderConfig::Anthropic(read_anthropic_llm_config(env)?),
        "bedrock" => LlmProviderConfig::Bedrock(read_bedrock_llm_config(env)?),
        "vertexai" | "vertex_ai" => LlmProviderConfig::VertexAi(read_vertex_ai_llm_config(env)?),
        _ => {
            return Err(EnvConfigError::InvalidValue {
                name: "LLM_PROVIDER".to_string(),
                value: provider,
            });
        }
    };

    Ok(LlmConfig {
        provider: provider_config,
    })
}

fn read_openai_llm_config(env: &dyn EnvironmentReader) -> Result<OpenAiLlmConfig, EnvConfigError> {
    Ok(OpenAiLlmConfig {
        api_key: read_required_env(env, "LLM_OPENAI_API_KEY")?,
        task_runner_model: DEFAULT_OPENAI_TASK_RUNNER_MODEL.to_string(),
    })
}

fn read_anthropic_llm_config(
    env: &dyn EnvironmentReader,
) -> Result<AnthropicLlmConfig, EnvConfigError> {
    let model = read_required_env(env, "LLM_ANTHROPIC_MODEL")?;
    if !SUPPORTED_ANTHROPIC_MODELS.contains(&model.as_str()) {
        return Err(EnvConfigError::InvalidValue {
            name: "LLM_ANTHROPIC_MODEL".to_string(),
            value: model,
        });
    }

    Ok(AnthropicLlmConfig {
        api_key: read_required_env(env, "LLM_ANTHROPIC_API_KEY")?,
        model,
    })
}

fn read_bedrock_llm_config(
    env: &dyn EnvironmentReader,
) -> Result<BedrockLlmConfig, EnvConfigError> {
    Ok(BedrockLlmConfig {
        model_id: read_required_env(env, "LLM_BEDROCK_MODEL_ID")?,
    })
}

fn read_vertex_ai_llm_config(
    env: &dyn EnvironmentReader,
) -> Result<VertexAiLlmConfig, EnvConfigError> {
    Ok(VertexAiLlmConfig {
        project_id: read_required_env(env, "GOOGLE_CLOUD_PROJECT")?,
        location: read_required_env(env, "GOOGLE_CLOUD_LOCATION")?,
        model_id: read_required_env(env, "LLM_VERTEX_AI_MODEL_ID")?,
    })
}

fn read_github_config(env: &dyn EnvironmentReader) -> Result<GitHubConfig, EnvConfigError> {
    let private_key = read_required_env(env, "GITHUB_APP_PRIVATE_KEY")?;
    let installation_id_raw = read_required_env(env, "GITHUB_APP_INSTALLATION_ID")?;

    Ok(GitHubConfig {
        url: read_or_default(env, "GITHUB_MCP_URL", DEFAULT_GITHUB_MCP_URL),
        app_id: read_required_env(env, "GITHUB_APP_ID")?,
        private_key: SecretString::new(private_key.replace("\\n", "\n")),
        installation_id: read_required_positive_u32(
            "GITHUB_APP_INSTALLATION_ID",
            &installation_id_raw,
        )?,
        scope_org: read_required_env(env, "GITHUB_SEARCH_SCOPE_ORG")?,
    })
}

fn read_required_env(env: &dyn EnvironmentReader, name: &str) -> Result<String, EnvConfigError> {
    read_optional_non_empty_env(env, name).ok_or_else(|| EnvConfigError::MissingRequired {
        name: name.to_string(),
    })
}

fn read_optional_non_empty_env(env: &dyn EnvironmentReader, name: &str) -> Option<String> {
    match env.get(name) {
        Some(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn read_or_default(env: &dyn EnvironmentReader, name: &str, default_value: &str) -> String {
    match env.get(name) {
        Some(value) => value,
        None => default_value.to_string(),
    }
}

fn read_required_positive_u32(name: &str, value: &str) -> Result<u32, EnvConfigError> {
    match value.parse::<u32>() {
        Ok(number) if number > 0 => Ok(number),
        _ => Err(EnvConfigError::InvalidValue {
            name: name.to_string(),
            value: value.to_string(),
        }),
    }
}

fn read_positive_u32(
    env: &dyn EnvironmentReader,
    name: &str,
    default_value: u32,
) -> Result<u32, EnvConfigError> {
    match env.get(name) {
        Some(value) if !value.is_empty() => read_required_positive_u32(name, &value),
        _ => Ok(default_value),
    }
}

fn read_port(
    env: &dyn EnvironmentReader,
    name: &str,
    default_value: u16,
) -> Result<u16, EnvConfigError> {
    let parsed = read_positive_u32(env, name, u32::from(default_value))?;
    if parsed > 65_535 {
        return Err(EnvConfigError::InvalidValue {
            name: name.to_string(),
            value: parsed.to_string(),
        });
    }

    u16::try_from(parsed).map_err(|_| EnvConfigError::InvalidValue {
        name: name.to_string(),
        value: parsed.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        DEFAULT_GITHUB_MCP_URL, DEFAULT_JOB_BACKOFF_MS, DEFAULT_JOB_MAX_RETRY,
        DEFAULT_OPENAI_TASK_RUNNER_MODEL, DEFAULT_WORKER_CONCURRENCY, EnvConfigError,
        LlmProviderConfig, MockEnvironmentReader, SecretString, SlackConnectionMode,
        load_app_config_with_env,
    };

    fn environment_reader_mock(overrides: &[(&str, &str)]) -> MockEnvironmentReader {
        let mut values = HashMap::from([
            ("SLACK_BOT_TOKEN".to_string(), "xoxb-test".to_string()),
            ("SLACK_APP_TOKEN".to_string(), "xapp-test-token".to_string()),
            (
                "SLACK_SIGNING_SECRET".to_string(),
                "signing-secret".to_string(),
            ),
            ("DATADOG_API_KEY".to_string(), "dd-api-key".to_string()),
            ("DATADOG_APP_KEY".to_string(), "dd-app-key".to_string()),
            ("LLM_PROVIDER".to_string(), "openai".to_string()),
            (
                "LLM_OPENAI_API_KEY".to_string(),
                "openai-api-key".to_string(),
            ),
            ("GITHUB_APP_ID".to_string(), "12345".to_string()),
            (
                "GITHUB_APP_PRIVATE_KEY".to_string(),
                "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----".to_string(),
            ),
            (
                "GITHUB_APP_INSTALLATION_ID".to_string(),
                "123456".to_string(),
            ),
            (
                "GITHUB_SEARCH_SCOPE_ORG".to_string(),
                "example-org".to_string(),
            ),
        ]);

        for (key, value) in overrides {
            values.insert((*key).to_string(), (*value).to_string());
        }

        let mut env = MockEnvironmentReader::new();
        env.expect_get()
            .returning(move |name| values.get(name).cloned());
        env
    }

    #[test]
    fn uses_fixed_worker_settings_even_when_env_vars_are_set() {
        let env = environment_reader_mock(&[
            ("WORKER_CONCURRENCY", "9"),
            ("JOB_MAX_RETRY", "99"),
            ("JOB_BACKOFF_MS", "9999"),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config with fixed defaults");

        assert_eq!(config.worker_concurrency, DEFAULT_WORKER_CONCURRENCY);
        assert_eq!(config.job_max_retry, DEFAULT_JOB_MAX_RETRY);
        assert_eq!(config.job_backoff_ms, DEFAULT_JOB_BACKOFF_MS);
    }

    #[test]
    fn loads_port_from_env() {
        let env = environment_reader_mock(&[("PORT", "3010")]);

        let config = load_app_config_with_env(&env).expect("load app config");

        assert_eq!(config.port, 3010);
    }

    #[test]
    fn loads_github_mcp_config() {
        let env = environment_reader_mock(&[]);

        let config = load_app_config_with_env(&env).expect("load app config");

        let github = config.github;
        assert_eq!(github.url, DEFAULT_GITHUB_MCP_URL);
        assert_eq!(github.app_id, "12345");
        assert_eq!(
            github.private_key.expose(),
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"
        );
        assert_eq!(github.installation_id, 123456);
        assert_eq!(github.scope_org, "example-org");
    }

    #[test]
    fn loads_openai_llm_config() {
        let env = environment_reader_mock(&[("LLM_OPENAI_TASK_RUNNER_MODEL", "custom-model")]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.api_key, "openai-api-key");
                assert_eq!(provider.task_runner_model, DEFAULT_OPENAI_TASK_RUNNER_MODEL);
            }
            LlmProviderConfig::Anthropic(_)
            | LlmProviderConfig::Bedrock(_)
            | LlmProviderConfig::VertexAi(_) => {
                panic!("expected openai provider")
            }
        }
    }

    #[test]
    fn loads_anthropic_llm_config() {
        let env = environment_reader_mock(&[
            ("LLM_PROVIDER", "anthropic"),
            ("LLM_ANTHROPIC_API_KEY", "anthropic-api-key"),
            ("LLM_ANTHROPIC_MODEL", "claude-sonnet-4-6"),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match config.llm.provider {
            LlmProviderConfig::Anthropic(provider) => {
                assert_eq!(provider.api_key, "anthropic-api-key");
                assert_eq!(provider.model, "claude-sonnet-4-6");
            }
            LlmProviderConfig::OpenAi(_)
            | LlmProviderConfig::Bedrock(_)
            | LlmProviderConfig::VertexAi(_) => {
                panic!("expected anthropic provider")
            }
        }
    }

    #[test]
    fn rejects_unsupported_anthropic_model() {
        let env = environment_reader_mock(&[
            ("LLM_PROVIDER", "anthropic"),
            ("LLM_ANTHROPIC_API_KEY", "anthropic-api-key"),
            ("LLM_ANTHROPIC_MODEL", "claude-3-5-haiku-latest"),
        ]);

        let error = load_app_config_with_env(&env).expect_err("invalid anthropic model");

        assert_eq!(
            error,
            EnvConfigError::InvalidValue {
                name: "LLM_ANTHROPIC_MODEL".to_string(),
                value: "claude-3-5-haiku-latest".to_string(),
            }
        );
    }

    #[test]
    fn loads_bedrock_llm_config() {
        let env = environment_reader_mock(&[
            ("LLM_PROVIDER", "bedrock"),
            (
                "LLM_BEDROCK_MODEL_ID",
                "anthropic.claude-3-7-sonnet-20250219-v1:0",
            ),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match config.llm.provider {
            LlmProviderConfig::Bedrock(provider) => {
                assert_eq!(
                    provider.model_id,
                    "anthropic.claude-3-7-sonnet-20250219-v1:0"
                );
            }
            LlmProviderConfig::OpenAi(_)
            | LlmProviderConfig::Anthropic(_)
            | LlmProviderConfig::VertexAi(_) => {
                panic!("expected bedrock provider")
            }
        }
    }

    #[test]
    fn loads_vertex_ai_llm_config() {
        let env = environment_reader_mock(&[
            ("LLM_PROVIDER", "vertexai"),
            ("GOOGLE_CLOUD_PROJECT", "example-project"),
            ("GOOGLE_CLOUD_LOCATION", "global"),
            ("LLM_VERTEX_AI_MODEL_ID", "gemini-2.5-pro"),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match config.llm.provider {
            LlmProviderConfig::VertexAi(provider) => {
                assert_eq!(provider.project_id, "example-project");
                assert_eq!(provider.location, "global");
                assert_eq!(provider.model_id, "gemini-2.5-pro");
            }
            LlmProviderConfig::OpenAi(_)
            | LlmProviderConfig::Anthropic(_)
            | LlmProviderConfig::Bedrock(_) => {
                panic!("expected vertexai provider")
            }
        }
    }

    #[test]
    fn rejects_invalid_llm_provider() {
        let env = environment_reader_mock(&[("LLM_PROVIDER", "invalid")]);

        let error = load_app_config_with_env(&env).expect_err("reject invalid provider");

        assert_eq!(
            error,
            super::EnvConfigError::InvalidValue {
                name: "LLM_PROVIDER".to_string(),
                value: "invalid".to_string(),
            }
        );
    }

    #[test]
    fn defaults_to_socket_mode_when_socket_mode_not_set() {
        let env = environment_reader_mock(&[]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match &config.slack_connection_mode {
            SlackConnectionMode::SocketMode { app_token } => {
                assert_eq!(app_token.expose(), "xapp-test-token");
            }
            SlackConnectionMode::Http => panic!("expected SocketMode"),
        }
        assert_eq!(
            config.slack_signing_secret,
            Some("signing-secret".to_string())
        );
    }

    #[test]
    fn enables_socket_mode_with_valid_app_token() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "true"),
            ("SLACK_APP_TOKEN", "xapp-test-token"),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match &config.slack_connection_mode {
            SlackConnectionMode::SocketMode { app_token } => {
                assert_eq!(app_token.expose(), "xapp-test-token");
            }
            SlackConnectionMode::Http => panic!("expected SocketMode"),
        }
    }

    #[test]
    fn default_socket_mode_requires_app_token() {
        let env = environment_reader_mock_without_keys(&[], &["SLACK_APP_TOKEN"]);

        let error = load_app_config_with_env(&env).expect_err("missing SLACK_APP_TOKEN");

        assert_eq!(
            error,
            super::EnvConfigError::MissingRequired {
                name: "SLACK_APP_TOKEN".to_string(),
            }
        );
    }

    #[test]
    fn socket_mode_requires_app_token() {
        let env = environment_reader_mock_without_keys(
            &[("SLACK_SOCKET_MODE", "true")],
            &["SLACK_APP_TOKEN"],
        );

        let error = load_app_config_with_env(&env).expect_err("missing SLACK_APP_TOKEN");

        assert_eq!(
            error,
            super::EnvConfigError::MissingRequired {
                name: "SLACK_APP_TOKEN".to_string(),
            }
        );
    }

    #[test]
    fn socket_mode_rejects_non_xapp_token() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "true"),
            ("SLACK_APP_TOKEN", "xoxb-wrong-prefix"),
        ]);

        let error = load_app_config_with_env(&env).expect_err("invalid prefix");

        assert_eq!(
            error,
            super::EnvConfigError::InvalidValue {
                name: "SLACK_APP_TOKEN".to_string(),
                value: "must start with xapp-".to_string(),
            }
        );
    }

    #[test]
    fn disables_socket_mode_when_explicitly_false() {
        let env = environment_reader_mock(&[("SLACK_SOCKET_MODE", "false")]);

        let config = load_app_config_with_env(&env).expect("load app config");

        assert_eq!(config.slack_connection_mode, SlackConnectionMode::Http);
        assert_eq!(
            config.slack_signing_secret,
            Some("signing-secret".to_string())
        );
    }

    #[test]
    fn socket_mode_does_not_require_signing_secret() {
        let env = environment_reader_mock_without_keys(
            &[
                ("SLACK_SOCKET_MODE", "true"),
                ("SLACK_APP_TOKEN", "xapp-test-token"),
            ],
            &["SLACK_SIGNING_SECRET"],
        );

        let config = load_app_config_with_env(&env).expect("load app config");

        assert!(config.slack_signing_secret.is_none());
    }

    #[test]
    fn socket_mode_treats_empty_signing_secret_as_none() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "true"),
            ("SLACK_APP_TOKEN", "xapp-test-token"),
            ("SLACK_SIGNING_SECRET", ""),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        assert!(config.slack_signing_secret.is_none());
    }

    #[test]
    fn socket_mode_treats_whitespace_signing_secret_as_none() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "true"),
            ("SLACK_APP_TOKEN", "xapp-test-token"),
            ("SLACK_SIGNING_SECRET", "   "),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        assert!(config.slack_signing_secret.is_none());
    }

    #[test]
    fn http_mode_requires_signing_secret() {
        let env = environment_reader_mock_without_keys(
            &[("SLACK_SOCKET_MODE", "false")],
            &["SLACK_SIGNING_SECRET"],
        );

        let error = load_app_config_with_env(&env).expect_err("missing signing secret");

        assert_eq!(
            error,
            super::EnvConfigError::MissingRequired {
                name: "SLACK_SIGNING_SECRET".to_string(),
            }
        );
    }

    #[test]
    fn http_mode_rejects_empty_signing_secret() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "false"),
            ("SLACK_SIGNING_SECRET", ""),
        ]);

        let error = load_app_config_with_env(&env).expect_err("empty signing secret");

        assert_eq!(
            error,
            super::EnvConfigError::MissingRequired {
                name: "SLACK_SIGNING_SECRET".to_string(),
            }
        );
    }

    #[test]
    fn http_mode_rejects_whitespace_signing_secret() {
        let env = environment_reader_mock(&[
            ("SLACK_SOCKET_MODE", "false"),
            ("SLACK_SIGNING_SECRET", "   "),
        ]);

        let error = load_app_config_with_env(&env).expect_err("whitespace signing secret");

        assert_eq!(
            error,
            super::EnvConfigError::MissingRequired {
                name: "SLACK_SIGNING_SECRET".to_string(),
            }
        );
    }

    #[test]
    fn slack_connection_mode_debug_masks_app_token() {
        let mode = SlackConnectionMode::SocketMode {
            app_token: SecretString::new("xapp-secret".to_string()),
        };

        let debug_output = format!("{mode:?}");

        assert!(!debug_output.contains("xapp-secret"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    fn environment_reader_mock_without_keys(
        overrides: &[(&str, &str)],
        remove_keys: &[&str],
    ) -> MockEnvironmentReader {
        let mut values = HashMap::from([
            ("SLACK_BOT_TOKEN".to_string(), "xoxb-test".to_string()),
            ("SLACK_APP_TOKEN".to_string(), "xapp-test-token".to_string()),
            (
                "SLACK_SIGNING_SECRET".to_string(),
                "signing-secret".to_string(),
            ),
            ("DATADOG_API_KEY".to_string(), "dd-api-key".to_string()),
            ("DATADOG_APP_KEY".to_string(), "dd-app-key".to_string()),
            ("LLM_PROVIDER".to_string(), "openai".to_string()),
            (
                "LLM_OPENAI_API_KEY".to_string(),
                "openai-api-key".to_string(),
            ),
            ("GITHUB_APP_ID".to_string(), "12345".to_string()),
            (
                "GITHUB_APP_PRIVATE_KEY".to_string(),
                "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----".to_string(),
            ),
            (
                "GITHUB_APP_INSTALLATION_ID".to_string(),
                "123456".to_string(),
            ),
            (
                "GITHUB_SEARCH_SCOPE_ORG".to_string(),
                "example-org".to_string(),
            ),
        ]);

        for key in remove_keys {
            values.remove(*key);
        }
        for (key, value) in overrides {
            values.insert((*key).to_string(), (*value).to_string());
        }

        let mut env = MockEnvironmentReader::new();
        env.expect_get()
            .returning(move |name| values.get(name).cloned());
        env
    }
}
