use std::io;
use std::path::{Path, PathBuf};

use super::ConfigError;
use super::env::{EnvironmentReader, ProcessEnvironment, read_required_secret};
use super::file::{
    AiBackendFileConfig, AiFileConfig, FileConfig, SlackFileConfig, parse_file_config,
};
use super::model::{
    AnthropicLlmConfig, AppConfig, BedrockLlmConfig, GitHubConfig, LlmConfig, LlmProviderConfig,
    OpenAiLlmConfig, SecretString, SlackConnectionMode, VertexAiLlmConfig,
};

const DEFAULT_WORKER_CONCURRENCY: u32 = 2;
const DEFAULT_JOB_MAX_RETRY: u32 = 2;
const DEFAULT_JOB_BACKOFF_MS: u64 = 1_000;
const SUPPORTED_CONFIG_VERSION: u32 = 1;
const SUPPORTED_ANTHROPIC_MODELS: &[&str] =
    &["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigLoadOptions {
    pub explicit_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigSource {
    ExplicitPath(PathBuf),
    CurrentDir(PathBuf),
}

trait ConfigFileAccess {
    fn current_dir(&self) -> Result<PathBuf, io::Error>;
    fn read_to_string(&self, path: &Path) -> Result<String, io::Error>;
}

struct StdConfigFileAccess;

impl ConfigFileAccess for StdConfigFileAccess {
    fn current_dir(&self) -> Result<PathBuf, io::Error> {
        std::env::current_dir()
    }

    fn read_to_string(&self, path: &Path) -> Result<String, io::Error> {
        std::fs::read_to_string(path)
    }
}

pub fn load_app_config(options: &ConfigLoadOptions) -> Result<AppConfig, ConfigError> {
    load_app_config_with(options, &ProcessEnvironment, &StdConfigFileAccess)
}

fn load_app_config_with(
    options: &ConfigLoadOptions,
    env: &dyn EnvironmentReader,
    access: &dyn ConfigFileAccess,
) -> Result<AppConfig, ConfigError> {
    let source = select_config_source(options, access)?;
    let file_config = load_file_config(&source, access)?;
    resolve_app_config(file_config, env)
}

fn select_config_source(
    options: &ConfigLoadOptions,
    access: &dyn ConfigFileAccess,
) -> Result<ConfigSource, ConfigError> {
    if let Some(path) = options.explicit_path.clone() {
        return Ok(ConfigSource::ExplicitPath(path));
    }

    let current_dir = access
        .current_dir()
        .map_err(|source| ConfigError::CurrentDir { source })?;
    Ok(ConfigSource::CurrentDir(current_dir.join("reili.toml")))
}

fn load_file_config(
    source: &ConfigSource,
    access: &dyn ConfigFileAccess,
) -> Result<FileConfig, ConfigError> {
    match source {
        ConfigSource::ExplicitPath(path) | ConfigSource::CurrentDir(path) => {
            let contents = access
                .read_to_string(path)
                .map_err(|source| ConfigError::ReadFile {
                    path: path.clone(),
                    source,
                })?;
            parse_file_config(path, &contents)
        }
    }
}

fn resolve_app_config(
    file_config: FileConfig,
    env: &dyn EnvironmentReader,
) -> Result<AppConfig, ConfigError> {
    validate_version(file_config.version)?;

    let slack_bot_token = read_required_secret(
        env,
        &file_config.channel.slack.auth.bot_token_env,
        "channel.slack.auth.bot_token_env",
    )?;
    let slack_resolution = resolve_slack_connection_mode(&file_config.channel.slack, env)?;
    let llm_provider = resolve_llm_provider(&file_config.ai, env)?;
    let github = resolve_github_config(&file_config, env)?;

    Ok(AppConfig {
        slack_bot_token,
        slack_signing_secret: slack_resolution.signing_secret,
        slack_connection_mode: slack_resolution.connection_mode,
        port: validate_port(file_config.server.port, "server.port")?,
        worker_concurrency: DEFAULT_WORKER_CONCURRENCY,
        job_max_retry: DEFAULT_JOB_MAX_RETRY,
        job_backoff_ms: DEFAULT_JOB_BACKOFF_MS,
        datadog_api_key: read_required_secret(
            env,
            &file_config.connector.datadog.api_key_env,
            "connector.datadog.api_key_env",
        )?,
        datadog_app_key: read_required_secret(
            env,
            &file_config.connector.datadog.app_key_env,
            "connector.datadog.app_key_env",
        )?,
        datadog_site: require_non_empty(
            &file_config.connector.datadog.site,
            "connector.datadog.site",
        )?,
        llm: LlmConfig {
            provider: llm_provider,
        },
        github,
        language: require_non_empty(&file_config.conversation.language, "conversation.language")?,
    })
}

fn validate_version(version: u32) -> Result<(), ConfigError> {
    if version == SUPPORTED_CONFIG_VERSION {
        return Ok(());
    }

    Err(ConfigError::UnsupportedVersion { found: version })
}

struct ResolvedSlackConfig {
    connection_mode: SlackConnectionMode,
    signing_secret: Option<SecretString>,
}

fn resolve_slack_connection_mode(
    slack: &SlackFileConfig,
    env: &dyn EnvironmentReader,
) -> Result<ResolvedSlackConfig, ConfigError> {
    let mode = resolve_slack_mode(slack)?;

    match mode.as_str() {
        "socket" => {
            let socket = slack
                .socket
                .as_ref()
                .ok_or_else(|| ConfigError::InvalidValue {
                    field: "channel.slack.socket".to_string(),
                    message: "is required when `channel.slack.mode` is `socket`".to_string(),
                })?;
            let app_token = read_required_secret(
                env,
                socket.app_token_env.as_deref().unwrap_or_default(),
                "channel.slack.socket.app_token_env",
            )?;
            if !app_token.expose().starts_with("xapp-") {
                return Err(ConfigError::InvalidValue {
                    field: "channel.slack.socket.app_token_env".to_string(),
                    message: "must resolve to a Slack App-Level Token starting with `xapp-`"
                        .to_string(),
                });
            }

            Ok(ResolvedSlackConfig {
                connection_mode: SlackConnectionMode::SocketMode { app_token },
                signing_secret: None,
            })
        }
        "http" => {
            let http = slack
                .http
                .as_ref()
                .ok_or_else(|| ConfigError::InvalidValue {
                    field: "channel.slack.http".to_string(),
                    message: "is required when `channel.slack.mode` is `http`".to_string(),
                })?;
            let signing_secret = read_required_secret(
                env,
                http.signing_secret_env.as_deref().unwrap_or_default(),
                "channel.slack.http.signing_secret_env",
            )?;

            Ok(ResolvedSlackConfig {
                connection_mode: SlackConnectionMode::Http,
                signing_secret: Some(signing_secret),
            })
        }
        _ => Err(ConfigError::InvalidValue {
            field: "channel.slack.mode".to_string(),
            message: format!("unsupported Slack mode `{mode}`"),
        }),
    }
}

fn resolve_slack_mode(slack: &SlackFileConfig) -> Result<String, ConfigError> {
    match (
        slack
            .mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        slack.socket_mode,
    ) {
        (Some(mode), Some(socket_mode)) => {
            let alias_mode = if socket_mode { "socket" } else { "http" };
            if mode != alias_mode {
                return Err(ConfigError::InvalidValue {
                    field: "channel.slack".to_string(),
                    message: format!(
                        "`mode = \"{mode}\"` conflicts with `socket_mode = {socket_mode}`"
                    ),
                });
            }
            Ok(mode.to_string())
        }
        (Some(mode), None) => Ok(mode.to_string()),
        (None, Some(true)) => Ok("socket".to_string()),
        (None, Some(false)) => Ok("http".to_string()),
        (None, None) => Err(ConfigError::InvalidValue {
            field: "channel.slack.mode".to_string(),
            message: "must be set to `socket` or `http`".to_string(),
        }),
    }
}

fn resolve_llm_provider(
    ai: &AiFileConfig,
    env: &dyn EnvironmentReader,
) -> Result<LlmProviderConfig, ConfigError> {
    let backend_id = require_non_empty(&ai.default_backend, "ai.default_backend")?;
    let backend = ai
        .backends
        .get(&backend_id)
        .ok_or_else(|| ConfigError::InvalidValue {
            field: "ai.default_backend".to_string(),
            message: format!(
                "references unknown backend `{backend_id}`; expected one of [{}]",
                ai.backends.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        })?;
    let backend_field_prefix = format!("ai.backends.{backend_id}");
    let provider = require_backend_field(
        backend.provider.as_deref(),
        &format!("{backend_field_prefix}.provider"),
    )?;

    match provider.as_str() {
        "openai" => resolve_openai_backend(backend, env, &backend_field_prefix),
        "anthropic" => resolve_anthropic_backend(backend, env, &backend_field_prefix),
        "bedrock" => resolve_bedrock_backend(backend, &backend_field_prefix),
        "vertexai" | "vertex_ai" => resolve_vertex_ai_backend(backend, &backend_field_prefix),
        _ => Err(ConfigError::InvalidValue {
            field: format!("{backend_field_prefix}.provider"),
            message: format!("unsupported provider `{provider}`"),
        }),
    }
}

fn resolve_openai_backend(
    backend: &AiBackendFileConfig,
    env: &dyn EnvironmentReader,
    prefix: &str,
) -> Result<LlmProviderConfig, ConfigError> {
    Ok(LlmProviderConfig::OpenAi(OpenAiLlmConfig {
        api_key: read_required_secret(
            env,
            backend.api_key_env.as_deref().unwrap_or_default(),
            &format!("{prefix}.api_key_env"),
        )?,
        task_runner_model: require_backend_field(
            backend.task_runner_model.as_deref(),
            &format!("{prefix}.task_runner_model"),
        )?,
    }))
}

fn resolve_anthropic_backend(
    backend: &AiBackendFileConfig,
    env: &dyn EnvironmentReader,
    prefix: &str,
) -> Result<LlmProviderConfig, ConfigError> {
    let model = require_backend_field(backend.model.as_deref(), &format!("{prefix}.model"))?;
    if !SUPPORTED_ANTHROPIC_MODELS.contains(&model.as_str()) {
        return Err(ConfigError::InvalidValue {
            field: format!("{prefix}.model"),
            message: format!(
                "unsupported Anthropic model `{model}`; expected one of [{}]",
                SUPPORTED_ANTHROPIC_MODELS.join(", ")
            ),
        });
    }

    Ok(LlmProviderConfig::Anthropic(AnthropicLlmConfig {
        api_key: read_required_secret(
            env,
            backend.api_key_env.as_deref().unwrap_or_default(),
            &format!("{prefix}.api_key_env"),
        )?,
        model,
    }))
}

fn resolve_bedrock_backend(
    backend: &AiBackendFileConfig,
    prefix: &str,
) -> Result<LlmProviderConfig, ConfigError> {
    Ok(LlmProviderConfig::Bedrock(BedrockLlmConfig {
        model_id: require_backend_field(
            backend.model_id.as_deref(),
            &format!("{prefix}.model_id"),
        )?,
        aws_profile: optional_trimmed(backend.aws_profile.as_deref()),
        aws_region: optional_trimmed(backend.aws_region.as_deref()),
    }))
}

fn resolve_vertex_ai_backend(
    backend: &AiBackendFileConfig,
    prefix: &str,
) -> Result<LlmProviderConfig, ConfigError> {
    Ok(LlmProviderConfig::VertexAi(VertexAiLlmConfig {
        project_id: require_backend_field(
            backend.project_id.as_deref(),
            &format!("{prefix}.project_id"),
        )?,
        location: require_backend_field(
            backend.location.as_deref(),
            &format!("{prefix}.location"),
        )?,
        model_id: require_backend_field(
            backend.model_id.as_deref(),
            &format!("{prefix}.model_id"),
        )?,
    }))
}

fn resolve_github_config(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
) -> Result<GitHubConfig, ConfigError> {
    let app_id = require_non_empty(
        &file_config.connector.github.app.app_id,
        "connector.github.app.app_id",
    )?;
    validate_positive_u64(&app_id, "connector.github.app.app_id")?;

    let installation_id_raw = require_non_empty(
        &file_config.connector.github.app.installation_id,
        "connector.github.app.installation_id",
    )?;

    Ok(GitHubConfig {
        url: require_non_empty(
            &file_config.connector.github.mcp_url,
            "connector.github.mcp_url",
        )?,
        app_id,
        private_key: normalize_multiline_secret(read_required_secret(
            env,
            &file_config.connector.github.app.private_key_env,
            "connector.github.app.private_key_env",
        )?),
        installation_id: validate_positive_u32(
            &installation_id_raw,
            "connector.github.app.installation_id",
        )?,
        scope_org: require_non_empty(
            &file_config.connector.github.search_scope_org,
            "connector.github.search_scope_org",
        )?,
    })
}

fn normalize_multiline_secret(secret: SecretString) -> SecretString {
    SecretString::new(secret.expose().replace("\\n", "\n"))
}

fn require_non_empty(value: &str, field: &str) -> Result<String, ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: "must not be empty".to_string(),
        });
    }

    Ok(trimmed.to_string())
}

fn require_backend_field(value: Option<&str>, field: &str) -> Result<String, ConfigError> {
    match value {
        Some(value) => require_non_empty(value, field),
        None => Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: "is required".to_string(),
        }),
    }
}

fn optional_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_port(value: u32, field: &str) -> Result<u16, ConfigError> {
    if value == 0 || value > 65_535 {
        return Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: format!("must be a positive integer between 1 and 65535, got `{value}`"),
        });
    }

    u16::try_from(value).map_err(|_| ConfigError::InvalidValue {
        field: field.to_string(),
        message: format!("must be a positive integer between 1 and 65535, got `{value}`"),
    })
}

fn validate_positive_u32(value: &str, field: &str) -> Result<u32, ConfigError> {
    match value.parse::<u32>() {
        Ok(number) if number > 0 => Ok(number),
        _ => Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: format!("must be a positive integer, got `{value}`"),
        }),
    }
}

fn validate_positive_u64(value: &str, field: &str) -> Result<u64, ConfigError> {
    match value.parse::<u64>() {
        Ok(number) if number > 0 => Ok(number),
        _ => Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: format!("must be a positive integer, got `{value}`"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Error, ErrorKind};
    use std::path::{Path, PathBuf};

    use super::{
        ConfigFileAccess, ConfigLoadOptions, ConfigSource, load_app_config_with,
        resolve_app_config, select_config_source,
    };
    use crate::config::ConfigError;
    use crate::config::env::EnvironmentReader;
    use crate::config::file::parse_file_config;
    use crate::config::model::{LlmProviderConfig, SlackConnectionMode};

    const TEST_PATH: &str = "/test/reili.toml";

    #[derive(Default)]
    struct FixedEnvironment {
        values: HashMap<String, String>,
    }

    impl FixedEnvironment {
        fn with_overrides(overrides: &[(&str, &str)]) -> Self {
            let mut values = HashMap::from([
                ("SLACK_BOT_TOKEN".to_string(), "xoxb-test".to_string()),
                ("SLACK_APP_TOKEN".to_string(), "xapp-test-token".to_string()),
                (
                    "SLACK_SIGNING_SECRET".to_string(),
                    "signing-secret".to_string(),
                ),
                ("DATADOG_API_KEY".to_string(), "dd-api-key".to_string()),
                ("DATADOG_APP_KEY".to_string(), "dd-app-key".to_string()),
                (
                    "LLM_OPENAI_API_KEY".to_string(),
                    "openai-api-key".to_string(),
                ),
                (
                    "LLM_ANTHROPIC_API_KEY".to_string(),
                    "anthropic-api-key".to_string(),
                ),
                (
                    "GITHUB_APP_PRIVATE_KEY".to_string(),
                    "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----"
                        .to_string(),
                ),
            ]);

            for (key, value) in overrides {
                values.insert((*key).to_string(), (*value).to_string());
            }

            Self { values }
        }
    }

    impl EnvironmentReader for FixedEnvironment {
        fn get(&self, name: &str) -> Option<String> {
            self.values.get(name).cloned()
        }
    }

    struct FakeConfigFileAccess {
        current_dir: PathBuf,
        files: HashMap<PathBuf, Result<String, ErrorKind>>,
    }

    impl FakeConfigFileAccess {
        fn new(current_dir: &str) -> Self {
            Self {
                current_dir: PathBuf::from(current_dir),
                files: HashMap::new(),
            }
        }

        fn with_file(mut self, path: &str, contents: &str) -> Self {
            self.files
                .insert(PathBuf::from(path), Ok(contents.to_string()));
            self
        }
    }

    impl ConfigFileAccess for FakeConfigFileAccess {
        fn current_dir(&self) -> Result<PathBuf, Error> {
            Ok(self.current_dir.clone())
        }

        fn read_to_string(&self, path: &Path) -> Result<String, Error> {
            match self.files.get(path) {
                Some(Ok(contents)) => Ok(contents.clone()),
                Some(Err(kind)) => Err(Error::from(*kind)),
                None => Err(Error::from(ErrorKind::NotFound)),
            }
        }
    }

    #[test]
    fn uses_explicit_config_path_when_present() {
        let access = FakeConfigFileAccess::new("/workspace")
            .with_file("/custom/reili.toml", &valid_openai_config());

        let source = select_config_source(
            &ConfigLoadOptions {
                explicit_path: Some(PathBuf::from("/custom/reili.toml")),
            },
            &access,
        )
        .expect("select source");

        assert_eq!(
            source,
            ConfigSource::ExplicitPath(PathBuf::from("/custom/reili.toml"))
        );
    }

    #[test]
    fn returns_error_if_explicit_config_is_missing() {
        let env = FixedEnvironment::with_overrides(&[]);
        let access = FakeConfigFileAccess::new("/workspace");

        let error = load_app_config_with(
            &ConfigLoadOptions {
                explicit_path: Some(PathBuf::from("/missing/reili.toml")),
            },
            &env,
            &access,
        )
        .expect_err("missing explicit config should fail");

        match error {
            ConfigError::ReadFile { path, source } => {
                assert_eq!(path, PathBuf::from("/missing/reili.toml"));
                assert_eq!(source.kind(), ErrorKind::NotFound);
            }
            other => panic!("expected read-file error, got {other}"),
        }
    }

    #[test]
    fn uses_current_dir_config_when_path_is_not_explicit() {
        let access = FakeConfigFileAccess::new("/workspace")
            .with_file("/workspace/reili.toml", &valid_openai_config());

        let source =
            select_config_source(&ConfigLoadOptions::default(), &access).expect("select source");

        assert_eq!(
            source,
            ConfigSource::CurrentDir(PathBuf::from("/workspace/reili.toml"))
        );
    }

    #[test]
    fn returns_error_if_current_dir_config_is_missing() {
        let env = FixedEnvironment::with_overrides(&[]);
        let access = FakeConfigFileAccess::new("/workspace");

        let error = load_app_config_with(&ConfigLoadOptions::default(), &env, &access)
            .expect_err("missing current-dir config should fail");

        match error {
            ConfigError::ReadFile { path, source } => {
                assert_eq!(path, PathBuf::from("/workspace/reili.toml"));
                assert_eq!(source.kind(), ErrorKind::NotFound);
            }
            other => panic!("expected read-file error, got {other}"),
        }
    }

    #[test]
    fn resolves_openai_backend_from_toml_and_env() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.api_key.expose(), "openai-api-key");
                assert_eq!(provider.task_runner_model, "gpt-5.3-codex");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn ignores_invalid_inactive_backend_blocks() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]
mode = "socket"

[channel.slack.auth]
bot_token_env = "SLACK_BOT_TOKEN"

[channel.slack.socket]
app_token_env = "SLACK_APP_TOKEN"

[ai]
default_backend = "primary"

[ai.backends.primary]
provider = "openai"
task_runner_model = "gpt-5.3-codex"
api_key_env = "LLM_OPENAI_API_KEY"

[ai.backends.unused]
provider = "unsupported-provider"

[connector.datadog]
site = "datadoghq.com"
api_key_env = "DATADOG_API_KEY"
app_key_env = "DATADOG_APP_KEY"

[connector.github]
mcp_url = "https://api.githubcopilot.com/mcp/"
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
private_key_env = "GITHUB_APP_PRIVATE_KEY"
"#,
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.llm.provider.provider_name(), "openai");
    }

    #[test]
    fn resolves_anthropic_backend_from_toml_and_env() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_anthropic_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::Anthropic(provider) => {
                assert_eq!(provider.api_key.expose(), "anthropic-api-key");
                assert_eq!(provider.model, "claude-sonnet-4-6");
            }
            _ => panic!("expected anthropic provider"),
        }
    }

    #[test]
    fn resolves_bedrock_backend_with_optional_profile_and_region() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_bedrock_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::Bedrock(provider) => {
                assert_eq!(
                    provider.model_id,
                    "anthropic.claude-3-7-sonnet-20250219-v1:0"
                );
                assert_eq!(provider.aws_profile.as_deref(), Some("prod-sso"));
                assert_eq!(provider.aws_region.as_deref(), Some("ap-northeast-1"));
            }
            _ => panic!("expected bedrock provider"),
        }
    }

    #[test]
    fn resolves_vertex_ai_backend_without_secret_lookup() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_vertex_ai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::VertexAi(provider) => {
                assert_eq!(provider.project_id, "example-project");
                assert_eq!(provider.location, "global");
                assert_eq!(provider.model_id, "gemini-2.5-pro");
            }
            _ => panic!("expected vertex ai provider"),
        }
    }

    #[test]
    fn requires_slack_app_token_only_in_socket_mode() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_http_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.slack_connection_mode, SlackConnectionMode::Http);
        assert_eq!(
            config
                .slack_signing_secret
                .as_ref()
                .expect("http signing secret")
                .expose(),
            "signing-secret"
        );
    }

    #[test]
    fn requires_slack_signing_secret_only_in_http_mode() {
        let env = FixedEnvironment::with_overrides(&[
            ("SLACK_APP_TOKEN", "xapp-test-token"),
            ("SLACK_SIGNING_SECRET", ""),
        ]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.slack_connection_mode {
            SlackConnectionMode::SocketMode { app_token } => {
                assert_eq!(app_token.expose(), "xapp-test-token");
            }
            SlackConnectionMode::Http => panic!("expected socket mode"),
        }
        assert!(config.slack_signing_secret.is_none());
    }

    #[test]
    fn preserves_github_private_key_newline_normalization() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config.github.private_key.expose(),
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"
        );
    }

    #[test]
    fn accepts_legacy_socket_mode_alias() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]
socket_mode = true

[channel.slack.auth]
bot_token_env = "SLACK_BOT_TOKEN"

[channel.slack.socket]
app_token_env = "SLACK_APP_TOKEN"

[ai]
default_backend = "primary"

[ai.backends.primary]
provider = "openai"
task_runner_model = "gpt-5.3-codex"
api_key_env = "LLM_OPENAI_API_KEY"

[connector.datadog]
site = "datadoghq.com"
api_key_env = "DATADOG_API_KEY"
app_key_env = "DATADOG_APP_KEY"

[connector.github]
mcp_url = "https://api.githubcopilot.com/mcp/"
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
private_key_env = "GITHUB_APP_PRIVATE_KEY"
"#,
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert!(matches!(
            config.slack_connection_mode,
            SlackConnectionMode::SocketMode { .. }
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config =
            parse_runtime_config(&valid_openai_config().replace("version = 1", "version = 2"));

        let error = resolve_app_config(file_config, &env).expect_err("unsupported version");

        match error {
            ConfigError::UnsupportedVersion { found } => assert_eq!(found, 2),
            other => panic!("expected unsupported-version error, got {other}"),
        }
    }

    #[test]
    fn rejects_unknown_slack_mode() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_openai_config().replace("mode = \"socket\"", "mode = \"webhook\""),
        );

        let error = resolve_app_config(file_config, &env).expect_err("invalid slack mode");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "channel.slack.mode");
                assert!(message.contains("unsupported Slack mode"));
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }

    #[test]
    fn rejects_unknown_default_backend() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "default_backend = \"primary\"",
            "default_backend = \"missing\"",
        ));

        let error = resolve_app_config(file_config, &env).expect_err("invalid backend");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "ai.default_backend");
                assert!(message.contains("unknown backend `missing`"));
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }

    fn parse_runtime_config(toml: &str) -> crate::config::file::FileConfig {
        parse_file_config(Path::new(TEST_PATH), toml).expect("parse runtime config")
    }

    fn valid_openai_config() -> String {
        r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]
mode = "socket"

[channel.slack.auth]
bot_token_env = "SLACK_BOT_TOKEN"

[channel.slack.socket]
app_token_env = "SLACK_APP_TOKEN"

[channel.slack.http]
signing_secret_env = "SLACK_SIGNING_SECRET"

[ai]
default_backend = "primary"

[ai.backends.primary]
provider = "openai"
task_runner_model = "gpt-5.3-codex"
api_key_env = "LLM_OPENAI_API_KEY"

[ai.backends.fast]
provider = "anthropic"
model = "claude-haiku-4-5"
api_key_env = "LLM_ANTHROPIC_API_KEY"

[connector.datadog]
site = "datadoghq.com"
api_key_env = "DATADOG_API_KEY"
app_key_env = "DATADOG_APP_KEY"

[connector.github]
mcp_url = "https://api.githubcopilot.com/mcp/"
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
private_key_env = "GITHUB_APP_PRIVATE_KEY"
"#
        .to_string()
    }

    fn valid_http_config() -> String {
        valid_openai_config().replace("mode = \"socket\"", "mode = \"http\"")
    }

    fn valid_anthropic_config() -> String {
        valid_openai_config()
            .replace(
                "default_backend = \"primary\"",
                "default_backend = \"fast\"",
            )
            .replace(
                "model = \"claude-haiku-4-5\"",
                "model = \"claude-sonnet-4-6\"",
            )
    }

    fn valid_bedrock_config() -> String {
        r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]
mode = "socket"

[channel.slack.auth]
bot_token_env = "SLACK_BOT_TOKEN"

[channel.slack.socket]
app_token_env = "SLACK_APP_TOKEN"

[ai]
default_backend = "bedrock"

[ai.backends.bedrock]
provider = "bedrock"
model_id = "anthropic.claude-3-7-sonnet-20250219-v1:0"
aws_profile = "prod-sso"
aws_region = "ap-northeast-1"

[connector.datadog]
site = "datadoghq.com"
api_key_env = "DATADOG_API_KEY"
app_key_env = "DATADOG_APP_KEY"

[connector.github]
mcp_url = "https://api.githubcopilot.com/mcp/"
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
private_key_env = "GITHUB_APP_PRIVATE_KEY"
"#
        .to_string()
    }

    fn valid_vertex_ai_config() -> String {
        r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]
mode = "socket"

[channel.slack.auth]
bot_token_env = "SLACK_BOT_TOKEN"

[channel.slack.socket]
app_token_env = "SLACK_APP_TOKEN"

[ai]
default_backend = "vertex"

[ai.backends.vertex]
provider = "vertexai"
project_id = "example-project"
location = "global"
model_id = "gemini-2.5-pro"

[connector.datadog]
site = "datadoghq.com"
api_key_env = "DATADOG_API_KEY"
app_key_env = "DATADOG_APP_KEY"

[connector.github]
mcp_url = "https://api.githubcopilot.com/mcp/"
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
private_key_env = "GITHUB_APP_PRIVATE_KEY"
"#
        .to_string()
    }
}
