use thiserror::Error;

const DEFAULT_APP_PORT: u16 = 3000;
const DEFAULT_WORKER_CONCURRENCY: u32 = 2;
const DEFAULT_DATADOG_SITE: &str = "datadoghq.com";
const DEFAULT_LANGUAGE: &str = "English";
const DEFAULT_JOB_MAX_RETRY: u32 = 2;
const DEFAULT_JOB_BACKOFF_MS: u64 = 1_000;
const DEFAULT_OPENAI_TASK_RUNNER_MODEL: &str = "gpt-5.3-codex";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAuthConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: String,
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
    Bedrock(BedrockLlmConfig),
}

impl LlmProviderConfig {
    #[must_use]
    pub fn provider_name(&self) -> &str {
        match self {
            Self::OpenAi(_) => "openai",
            Self::Bedrock(_) => "bedrock",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiLlmConfig {
    pub api_key: String,
    pub task_runner_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BedrockLlmConfig {
    pub region: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubAppConfig {
    pub app_id: String,
    pub private_key: String,
    pub installation_id: u32,
    pub scope_org: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: String,
    pub port: u16,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub datadog_api_key: String,
    pub datadog_app_key: String,
    pub datadog_site: String,
    pub llm: LlmConfig,
    pub github: GitHubAppConfig,
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
    let slack_auth = read_slack_auth_config(env)?;
    let task_config = read_task_config(env)?;

    Ok(AppConfig {
        slack_bot_token: slack_auth.slack_bot_token,
        slack_signing_secret: slack_auth.slack_signing_secret,
        port: read_port(env, "PORT", DEFAULT_APP_PORT)?,
        worker_concurrency: DEFAULT_WORKER_CONCURRENCY,
        job_max_retry: DEFAULT_JOB_MAX_RETRY,
        job_backoff_ms: DEFAULT_JOB_BACKOFF_MS,
        datadog_api_key: task_config.datadog_api_key,
        datadog_app_key: task_config.datadog_app_key,
        datadog_site: task_config.datadog_site,
        llm: task_config.llm,
        github: read_github_app_config(env)?,
        language: task_config.language,
    })
}

fn read_slack_auth_config(env: &dyn EnvironmentReader) -> Result<SlackAuthConfig, EnvConfigError> {
    Ok(SlackAuthConfig {
        slack_bot_token: read_required_env(env, "SLACK_BOT_TOKEN")?,
        slack_signing_secret: read_required_env(env, "SLACK_SIGNING_SECRET")?,
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
        "bedrock" => LlmProviderConfig::Bedrock(read_bedrock_llm_config(env)?),
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

fn read_bedrock_llm_config(
    env: &dyn EnvironmentReader,
) -> Result<BedrockLlmConfig, EnvConfigError> {
    Ok(BedrockLlmConfig {
        region: read_required_env(env, "LLM_BEDROCK_REGION")?,
        model_id: read_required_env(env, "LLM_BEDROCK_MODEL_ID")?,
    })
}

fn read_github_app_config(env: &dyn EnvironmentReader) -> Result<GitHubAppConfig, EnvConfigError> {
    let private_key = read_required_env(env, "GITHUB_APP_PRIVATE_KEY")?;
    let installation_id_raw = read_required_env(env, "GITHUB_APP_INSTALLATION_ID")?;

    Ok(GitHubAppConfig {
        app_id: read_required_env(env, "GITHUB_APP_ID")?,
        private_key: private_key.replace("\\n", "\n"),
        installation_id: read_required_positive_u32(
            "GITHUB_APP_INSTALLATION_ID",
            &installation_id_raw,
        )?,
        scope_org: read_required_env(env, "GITHUB_SEARCH_SCOPE_ORG")?,
    })
}

fn read_required_env(env: &dyn EnvironmentReader, name: &str) -> Result<String, EnvConfigError> {
    match env.get(name) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(EnvConfigError::MissingRequired {
            name: name.to_string(),
        }),
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
        DEFAULT_JOB_BACKOFF_MS, DEFAULT_JOB_MAX_RETRY, DEFAULT_OPENAI_TASK_RUNNER_MODEL,
        DEFAULT_WORKER_CONCURRENCY, LlmProviderConfig, MockEnvironmentReader,
        load_app_config_with_env,
    };

    fn environment_reader_mock(overrides: &[(&str, &str)]) -> MockEnvironmentReader {
        let mut values = HashMap::from([
            ("SLACK_BOT_TOKEN".to_string(), "xoxb-test".to_string()),
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
    fn converts_multiline_github_private_key() {
        let env = environment_reader_mock(&[]);

        let config = load_app_config_with_env(&env).expect("load app config");

        assert!(config.github.private_key.contains('\n'));
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
            LlmProviderConfig::Bedrock(_) => panic!("expected openai provider"),
        }
    }

    #[test]
    fn loads_bedrock_llm_config() {
        let env = environment_reader_mock(&[
            ("LLM_PROVIDER", "bedrock"),
            ("LLM_BEDROCK_REGION", "ap-northeast-1"),
            (
                "LLM_BEDROCK_MODEL_ID",
                "anthropic.claude-3-7-sonnet-20250219-v1:0",
            ),
        ]);

        let config = load_app_config_with_env(&env).expect("load app config");

        match config.llm.provider {
            LlmProviderConfig::Bedrock(provider) => {
                assert_eq!(provider.region, "ap-northeast-1");
                assert_eq!(
                    provider.model_id,
                    "anthropic.claude-3-7-sonnet-20250219-v1:0"
                );
            }
            LlmProviderConfig::OpenAi(_) => panic!("expected bedrock provider"),
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
}
