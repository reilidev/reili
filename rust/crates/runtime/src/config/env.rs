use thiserror::Error;

const DEFAULT_INGRESS_PORT: u16 = 3000;
const DEFAULT_WORKER_PORT: u16 = 3100;
const DEFAULT_WORKER_CONCURRENCY: u32 = 2;
const DEFAULT_DATADOG_SITE: &str = "datadoghq.com";
const DEFAULT_LANGUAGE: &str = "English";
const DEFAULT_JOB_MAX_RETRY: u32 = 2;
const DEFAULT_JOB_BACKOFF_MS: u64 = 1_000;
const DEFAULT_WORKER_DISPATCH_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_OPENAI_WEB_SEARCH_MODEL: &str = "gpt-5.4";
const DEFAULT_OPENAI_WEB_SEARCH_TIMEOUT_MS: u64 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackAuthConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationConfig {
    pub datadog_api_key: String,
    pub datadog_app_key: String,
    pub datadog_site: String,
    pub openai_api_key: String,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubAppConfig {
    pub app_id: String,
    pub private_key: String,
    pub installation_id: u32,
    pub scope_org: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: String,
    pub port: u16,
    pub worker_base_url: String,
    pub worker_internal_token: String,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub worker_dispatch_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiWebSearchConfig {
    pub model: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    pub slack_bot_token: String,
    pub slack_signing_secret: String,
    pub datadog_api_key: String,
    pub datadog_app_key: String,
    pub datadog_site: String,
    pub openai_api_key: String,
    pub language: String,
    pub worker_internal_port: u16,
    pub worker_internal_token: String,
    pub worker_concurrency: u32,
    pub job_max_retry: u32,
    pub job_backoff_ms: u64,
    pub github: GitHubAppConfig,
    pub openai_web_search: OpenAiWebSearchConfig,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvConfigError {
    #[error("Missing required environment variable: {name}")]
    MissingRequired { name: String },
    #[error("Invalid {name} value: {value}")]
    InvalidValue { name: String, value: String },
}

pub fn load_ingress_config() -> Result<IngressConfig, EnvConfigError> {
    load_ingress_config_with_env(&ProcessEnvironment)
}

pub fn load_worker_config() -> Result<WorkerConfig, EnvConfigError> {
    load_worker_config_with_env(&ProcessEnvironment)
}

trait EnvironmentReader {
    fn get(&self, name: &str) -> Option<String>;
}

struct ProcessEnvironment;

impl EnvironmentReader for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

fn load_ingress_config_with_env(
    env: &dyn EnvironmentReader,
) -> Result<IngressConfig, EnvConfigError> {
    let slack_auth = read_slack_auth_config(env)?;

    Ok(IngressConfig {
        slack_bot_token: slack_auth.slack_bot_token,
        slack_signing_secret: slack_auth.slack_signing_secret,
        port: read_port(env, "PORT", DEFAULT_INGRESS_PORT)?,
        worker_base_url: read_required_env(env, "WORKER_BASE_URL")?,
        worker_internal_token: read_required_env(env, "WORKER_INTERNAL_TOKEN")?,
        job_max_retry: DEFAULT_JOB_MAX_RETRY,
        job_backoff_ms: DEFAULT_JOB_BACKOFF_MS,
        worker_dispatch_timeout_ms: DEFAULT_WORKER_DISPATCH_TIMEOUT_MS,
    })
}

fn load_worker_config_with_env(
    env: &dyn EnvironmentReader,
) -> Result<WorkerConfig, EnvConfigError> {
    let slack_auth = read_slack_auth_config(env)?;
    let investigation_config = read_investigation_config(env)?;

    Ok(WorkerConfig {
        slack_bot_token: slack_auth.slack_bot_token,
        slack_signing_secret: slack_auth.slack_signing_secret,
        datadog_api_key: investigation_config.datadog_api_key,
        datadog_app_key: investigation_config.datadog_app_key,
        datadog_site: investigation_config.datadog_site,
        openai_api_key: investigation_config.openai_api_key,
        language: investigation_config.language,
        worker_internal_port: read_port(env, "WORKER_INTERNAL_PORT", DEFAULT_WORKER_PORT)?,
        worker_internal_token: read_required_env(env, "WORKER_INTERNAL_TOKEN")?,
        worker_concurrency: DEFAULT_WORKER_CONCURRENCY,
        job_max_retry: DEFAULT_JOB_MAX_RETRY,
        job_backoff_ms: DEFAULT_JOB_BACKOFF_MS,
        github: read_github_app_config(env)?,
        openai_web_search: read_openai_web_search_config(env),
    })
}

fn read_slack_auth_config(env: &dyn EnvironmentReader) -> Result<SlackAuthConfig, EnvConfigError> {
    Ok(SlackAuthConfig {
        slack_bot_token: read_required_env(env, "SLACK_BOT_TOKEN")?,
        slack_signing_secret: read_required_env(env, "SLACK_SIGNING_SECRET")?,
    })
}

fn read_investigation_config(
    env: &dyn EnvironmentReader,
) -> Result<InvestigationConfig, EnvConfigError> {
    Ok(InvestigationConfig {
        datadog_api_key: read_required_env(env, "DATADOG_API_KEY")?,
        datadog_app_key: read_required_env(env, "DATADOG_APP_KEY")?,
        datadog_site: read_or_default(env, "DATADOG_SITE", DEFAULT_DATADOG_SITE),
        openai_api_key: read_required_env(env, "OPENAI_API_KEY")?,
        language: read_or_default(env, "LANGUAGE", DEFAULT_LANGUAGE),
    })
}

fn read_github_app_config(env: &dyn EnvironmentReader) -> Result<GitHubAppConfig, EnvConfigError> {
    let private_key = read_required_env(env, "GITHUB_APP_PRIVATE_KEY")?;
    let installation_id_raw = read_required_env(env, "GITHUB_APP_INSTALLATION_ID")?;

    Ok(GitHubAppConfig {
        app_id: read_required_env(env, "GITHUB_APP_ID")?,
        private_key: private_key.replace("\\n", "\n"),
        installation_id: read_required_positive_int(
            "GITHUB_APP_INSTALLATION_ID",
            &installation_id_raw,
        )?,
        scope_org: read_required_env(env, "GITHUB_SEARCH_SCOPE_ORG")?,
    })
}

fn read_openai_web_search_config(env: &dyn EnvironmentReader) -> OpenAiWebSearchConfig {
    OpenAiWebSearchConfig {
        model: read_or_default(
            env,
            "OPENAI_WEB_SEARCH_MODEL",
            DEFAULT_OPENAI_WEB_SEARCH_MODEL,
        ),
        timeout_ms: env
            .get("OPENAI_WEB_SEARCH_TIMEOUT_MS")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_OPENAI_WEB_SEARCH_TIMEOUT_MS),
    }
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

fn read_required_positive_int(name: &str, value: &str) -> Result<u32, EnvConfigError> {
    parse_positive_u32(name, value)
}

fn read_positive_int(
    env: &dyn EnvironmentReader,
    name: &str,
    default_value: u32,
) -> Result<u32, EnvConfigError> {
    match env.get(name) {
        Some(value) if !value.is_empty() => parse_positive_u32(name, &value),
        _ => Ok(default_value),
    }
}

fn read_port(
    env: &dyn EnvironmentReader,
    name: &str,
    default_value: u16,
) -> Result<u16, EnvConfigError> {
    let parsed = read_positive_int(env, name, u32::from(default_value))?;
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

fn parse_positive_u32(name: &str, value: &str) -> Result<u32, EnvConfigError> {
    match value.parse::<u32>() {
        Ok(number) if number > 0 => Ok(number),
        _ => Err(EnvConfigError::InvalidValue {
            name: name.to_string(),
            value: value.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_JOB_BACKOFF_MS, DEFAULT_JOB_MAX_RETRY, DEFAULT_WORKER_DISPATCH_TIMEOUT_MS,
    };
    use super::{EnvironmentReader, load_ingress_config_with_env, load_worker_config_with_env};
    use std::collections::HashMap;

    struct MapEnvironment {
        values: HashMap<String, String>,
    }

    impl MapEnvironment {
        fn from_overrides(overrides: &[(&str, &str)]) -> Self {
            let mut values = HashMap::from([
                ("SLACK_BOT_TOKEN".to_string(), "xoxb-test".to_string()),
                (
                    "SLACK_SIGNING_SECRET".to_string(),
                    "signing-secret".to_string(),
                ),
                (
                    "WORKER_BASE_URL".to_string(),
                    "http://localhost:3100".to_string(),
                ),
                (
                    "WORKER_INTERNAL_TOKEN".to_string(),
                    "internal-token".to_string(),
                ),
                ("DATADOG_API_KEY".to_string(), "dd-api-key".to_string()),
                ("DATADOG_APP_KEY".to_string(), "dd-app-key".to_string()),
                ("OPENAI_API_KEY".to_string(), "openai-api-key".to_string()),
                ("GITHUB_APP_ID".to_string(), "12345".to_string()),
                (
                    "GITHUB_APP_PRIVATE_KEY".to_string(),
                    "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----"
                        .to_string(),
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

            Self { values }
        }
    }

    impl EnvironmentReader for MapEnvironment {
        fn get(&self, name: &str) -> Option<String> {
            self.values.get(name).cloned()
        }
    }

    #[test]
    fn uses_fixed_retry_settings_for_ingress_even_when_env_vars_are_set() {
        let env = MapEnvironment::from_overrides(&[
            ("JOB_MAX_RETRY", "99"),
            ("JOB_BACKOFF_MS", "9999"),
            ("WORKER_DISPATCH_TIMEOUT_MS", "9999"),
        ]);

        let config =
            load_ingress_config_with_env(&env).expect("load ingress config with fixed defaults");

        assert_eq!(config.job_max_retry, DEFAULT_JOB_MAX_RETRY);
        assert_eq!(config.job_backoff_ms, DEFAULT_JOB_BACKOFF_MS);
        assert_eq!(
            config.worker_dispatch_timeout_ms,
            DEFAULT_WORKER_DISPATCH_TIMEOUT_MS
        );
    }

    #[test]
    fn uses_fixed_worker_settings_even_when_env_vars_are_set() {
        let env = MapEnvironment::from_overrides(&[
            ("WORKER_CONCURRENCY", "9"),
            ("JOB_MAX_RETRY", "99"),
            ("JOB_BACKOFF_MS", "9999"),
        ]);

        let config =
            load_worker_config_with_env(&env).expect("load worker config with fixed defaults");

        assert_eq!(config.worker_concurrency, 2);
        assert_eq!(config.job_max_retry, DEFAULT_JOB_MAX_RETRY);
        assert_eq!(config.job_backoff_ms, DEFAULT_JOB_BACKOFF_MS);
    }
}
