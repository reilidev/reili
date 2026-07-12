use std::io;
use std::path::{Path, PathBuf};

use super::ConfigError;
use super::env::{EnvironmentReader, ProcessEnvironment, read_required_secret};
use super::file::{
    AiBackendFileConfig, AiFileConfig, FileConfig, SlackAuthorizationFileConfig,
    SlackChannelFileConfig, SlackFileConfig, parse_file_config,
};
use super::model::{
    AnthropicLlmConfig, AppConfig, BedrockLlmConfig, EsaConfig, GitHubConfig, JiraConfig,
    JudgeProviderConfig, LlmConfig, LlmProviderConfig, OpenAiLlmConfig, SlackAuthorizationActors,
    SlackAuthorizationConfig, SlackCanvasMemoryConfig, SlackChannelConfig, SlackConnectionMode,
    VertexAiLlmConfig,
};
use crate::config::SecretString;
use reili_core::messaging::slack::SlackChannelNamePattern;

const DEFAULT_WORKER_CONCURRENCY: u32 = 8;
const DEFAULT_JOB_MAX_RETRY: u32 = 2;
const DEFAULT_JOB_BACKOFF_MS: u64 = 1_000;
const SUPPORTED_CONFIG_VERSION: u32 = 1;
const DEFAULT_OPENAI_API_KEY_ENV: &str = "LLM_OPENAI_API_KEY";
const DEFAULT_ANTHROPIC_API_KEY_ENV: &str = "LLM_ANTHROPIC_API_KEY";
const DEFAULT_OPENAI_REASONING_EFFORT: &str = "medium";

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
    let slack_authorization =
        resolve_slack_authorization(file_config.channel.slack.authorization.as_ref());
    let slack_channels = resolve_slack_channels(&file_config.channel.slack.channels)?;
    let llm_provider = resolve_llm_provider(&file_config.ai, env)?;
    let judge_llm = if slack_channels.iter().any(|channel| channel.auto_response) {
        Some(resolve_judge_llm_provider(&file_config.ai, env)?)
    } else {
        None
    };
    let github = resolve_github_config(&file_config, env)?;
    let esa = resolve_esa_config(&file_config, env)?;
    let jira = resolve_jira_config(&file_config, env)?;
    let memory = resolve_memory_config(&file_config);

    Ok(AppConfig {
        slack_bot_token,
        slack_signing_secret: slack_resolution.signing_secret,
        slack_connection_mode: slack_resolution.connection_mode,
        slack_authorization,
        slack_channels,
        port: file_config.server.port,
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
        datadog_site: file_config.connector.datadog.site,
        llm: LlmConfig {
            provider: llm_provider,
        },
        judge_llm,
        github,
        esa,
        jira,
        memory,
        language: file_config.conversation.language,
        additional_system_prompt: optional_trimmed(
            file_config.conversation.additional_system_prompt.as_deref(),
        ),
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
    if slack.socket_mode {
        let app_token = read_required_secret(
            env,
            &slack.socket.app_token_env,
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
    } else {
        let signing_secret = read_required_secret(
            env,
            &slack.http.signing_secret_env,
            "channel.slack.http.signing_secret_env",
        )?;

        Ok(ResolvedSlackConfig {
            connection_mode: SlackConnectionMode::Http,
            signing_secret: Some(signing_secret),
        })
    }
}

fn resolve_slack_authorization(
    authorization: Option<&SlackAuthorizationFileConfig>,
) -> Option<SlackAuthorizationConfig> {
    // `[channel.slack.authorization]` only carries actor conditions; channel conditions live in
    // `[[channel.slack.channels]]`.
    let actors = authorization?.actors.as_ref();
    let user_ids = actors.and_then(|a| a.user_ids.clone());
    let user_group_ids = actors.and_then(|a| a.user_group_ids.clone());
    let allow_bot = actors.is_some_and(|a| a.allow_bot);

    Some(SlackAuthorizationConfig {
        actors: if user_ids.is_some() || user_group_ids.is_some() || allow_bot {
            Some(SlackAuthorizationActors {
                user_ids,
                user_group_ids,
                allow_bot,
            })
        } else {
            None
        },
    })
}

fn resolve_slack_channels(
    channels: &[SlackChannelFileConfig],
) -> Result<Vec<SlackChannelConfig>, ConfigError> {
    channels
        .iter()
        .enumerate()
        .map(|(index, channel)| {
            let field = format!("channel.slack.channels[{index}]");
            if !channel.mention && !channel.auto_response {
                return Err(ConfigError::InvalidValue {
                    field,
                    message: "entry disables both `mention` and `auto_response`; remove it or enable at least one".to_string(),
                });
            }
            Ok(SlackChannelConfig {
                names: channel
                    .names
                    .iter()
                    .cloned()
                    .map(SlackChannelNamePattern::new)
                    .collect(),
                mention: channel.mention,
                auto_response: channel.auto_response,
                auto_response_policy: channel.auto_response_policy.clone(),
            })
        })
        .collect()
}

fn resolve_llm_provider(
    ai: &AiFileConfig,
    env: &dyn EnvironmentReader,
) -> Result<LlmProviderConfig, ConfigError> {
    let (lead_id, lead_field) = select_backend_id(
        ai.lead_backend.as_deref(),
        &ai.default_backend,
        "ai.lead_backend",
    );
    let (sub_id, sub_field) = select_backend_id(
        ai.sub_agent_backend.as_deref(),
        &ai.default_backend,
        "ai.sub_agent_backend",
    );

    let lead_backend = lookup_backend(ai, lead_id, lead_field)?;
    let sub_backend = lookup_backend(ai, sub_id, sub_field)?;

    let lead_provider = backend_provider_name(lead_backend);
    let sub_provider = backend_provider_name(sub_backend);
    if lead_provider != sub_provider {
        return Err(ConfigError::InvalidValue {
            field: sub_field.to_string(),
            message: format!(
                "backend `{sub_id}` uses provider `{sub_provider}`, but the lead backend `{lead_id}` uses provider `{lead_provider}`; the lead and sub-agent must share the same provider"
            ),
        });
    }

    let sub_agent_model = backend_model(sub_backend).to_string();
    let backend_field_prefix = format!("ai.backends.{lead_id}");

    match lead_backend {
        AiBackendFileConfig::OpenAi {
            model,
            api_key_env,
            reasoning_effort,
        } => resolve_openai_backend(ResolveOpenAiBackendInput {
            model,
            sub_agent_model,
            api_key_env: api_key_env.as_deref(),
            reasoning_effort: reasoning_effort.as_deref(),
            env,
            prefix: &backend_field_prefix,
        }),
        AiBackendFileConfig::Anthropic { model, api_key_env } => {
            Ok(LlmProviderConfig::Anthropic(AnthropicLlmConfig {
                api_key: resolve_anthropic_api_key(
                    env,
                    api_key_env.as_deref(),
                    &backend_field_prefix,
                )?,
                model: model.to_string(),
                sub_agent_model,
            }))
        }
        AiBackendFileConfig::Bedrock {
            model_id,
            aws_profile,
            aws_region,
        } => Ok(LlmProviderConfig::Bedrock(BedrockLlmConfig {
            model_id: model_id.to_string(),
            sub_agent_model_id: sub_agent_model,
            aws_profile: optional_trimmed(aws_profile.as_deref()),
            aws_region: optional_trimmed(aws_region.as_deref()),
        })),
        AiBackendFileConfig::VertexAi {
            project_id,
            location,
            model_id,
        } => Ok(LlmProviderConfig::VertexAi(VertexAiLlmConfig {
            project_id: project_id.to_string(),
            location: location.to_string(),
            model_id: model_id.to_string(),
            sub_agent_model_id: sub_agent_model,
        })),
    }
}

/// Resolve the backend used by the auto-response judge. The judge is a
/// single-shot call independent of the lead/sub-agent pair, so it may use a
/// different provider than the task runner and carries no sub-agent model.
fn resolve_judge_llm_provider(
    ai: &AiFileConfig,
    env: &dyn EnvironmentReader,
) -> Result<JudgeProviderConfig, ConfigError> {
    let (judge_id, judge_field) = select_backend_id(
        ai.judge_backend.as_deref(),
        &ai.default_backend,
        "ai.judge_backend",
    );
    let judge_backend = lookup_backend(ai, judge_id, judge_field)?;
    let prefix = format!("ai.backends.{judge_id}");

    match judge_backend {
        AiBackendFileConfig::OpenAi {
            model, api_key_env, ..
        } => Ok(JudgeProviderConfig::OpenAi {
            api_key: resolve_openai_api_key(env, api_key_env.as_deref(), &prefix)?,
            model: model.to_string(),
        }),
        AiBackendFileConfig::Anthropic { model, api_key_env } => {
            Ok(JudgeProviderConfig::Anthropic {
                api_key: resolve_anthropic_api_key(env, api_key_env.as_deref(), &prefix)?,
                model: model.to_string(),
            })
        }
        AiBackendFileConfig::Bedrock {
            model_id,
            aws_profile,
            aws_region,
        } => Ok(JudgeProviderConfig::Bedrock {
            model_id: model_id.to_string(),
            aws_profile: optional_trimmed(aws_profile.as_deref()),
            aws_region: optional_trimmed(aws_region.as_deref()),
        }),
        AiBackendFileConfig::VertexAi {
            project_id,
            location,
            model_id,
        } => Ok(JudgeProviderConfig::VertexAi {
            project_id: project_id.to_string(),
            location: location.to_string(),
            model_id: model_id.to_string(),
        }),
    }
}

/// Read an OpenAI backend's API key, falling back to the default env var.
fn resolve_openai_api_key(
    env: &dyn EnvironmentReader,
    api_key_env: Option<&str>,
    prefix: &str,
) -> Result<SecretString, ConfigError> {
    read_required_secret(
        env,
        api_key_env.unwrap_or(DEFAULT_OPENAI_API_KEY_ENV),
        &format!("{prefix}.api_key_env"),
    )
}

/// Read an Anthropic backend's API key, falling back to the default env var.
fn resolve_anthropic_api_key(
    env: &dyn EnvironmentReader,
    api_key_env: Option<&str>,
    prefix: &str,
) -> Result<SecretString, ConfigError> {
    read_required_secret(
        env,
        api_key_env.unwrap_or(DEFAULT_ANTHROPIC_API_KEY_ENV),
        &format!("{prefix}.api_key_env"),
    )
}

/// Pick the backend id for an agent role, falling back to `default_backend`.
///
/// Returns the resolved id alongside the config field that named it, so error
/// messages point at the field the operator actually set.
fn select_backend_id<'a>(
    override_id: Option<&'a str>,
    default_backend: &'a str,
    override_field: &'static str,
) -> (&'a str, &'static str) {
    match override_id {
        Some(id) => (id, override_field),
        None => (default_backend, "ai.default_backend"),
    }
}

fn lookup_backend<'a>(
    ai: &'a AiFileConfig,
    backend_id: &str,
    field: &str,
) -> Result<&'a AiBackendFileConfig, ConfigError> {
    ai.backends
        .get(backend_id)
        .ok_or_else(|| ConfigError::InvalidValue {
            field: field.to_string(),
            message: format!(
                "references unknown backend `{backend_id}`; expected one of [{}]",
                ai.backends.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        })
}

fn backend_provider_name(backend: &AiBackendFileConfig) -> &'static str {
    match backend {
        AiBackendFileConfig::OpenAi { .. } => "openai",
        AiBackendFileConfig::Anthropic { .. } => "anthropic",
        AiBackendFileConfig::Bedrock { .. } => "bedrock",
        AiBackendFileConfig::VertexAi { .. } => "vertexai",
    }
}

fn backend_model(backend: &AiBackendFileConfig) -> &str {
    match backend {
        AiBackendFileConfig::OpenAi { model, .. }
        | AiBackendFileConfig::Anthropic { model, .. } => model,
        AiBackendFileConfig::Bedrock { model_id, .. }
        | AiBackendFileConfig::VertexAi { model_id, .. } => model_id,
    }
}

struct ResolveOpenAiBackendInput<'a> {
    model: &'a str,
    sub_agent_model: String,
    api_key_env: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
    env: &'a dyn EnvironmentReader,
    prefix: &'a str,
}

fn resolve_openai_backend(
    input: ResolveOpenAiBackendInput<'_>,
) -> Result<LlmProviderConfig, ConfigError> {
    let reasoning_effort = input
        .reasoning_effort
        .unwrap_or(DEFAULT_OPENAI_REASONING_EFFORT);

    Ok(LlmProviderConfig::OpenAi(OpenAiLlmConfig {
        api_key: resolve_openai_api_key(input.env, input.api_key_env, input.prefix)?,
        model: input.model.to_string(),
        sub_agent_model: input.sub_agent_model,
        reasoning_effort: reasoning_effort.to_string(),
    }))
}

fn resolve_github_config(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
) -> Result<GitHubConfig, ConfigError> {
    Ok(GitHubConfig {
        url: file_config.connector.github.mcp_url.clone(),
        app_id: file_config.connector.github.app.app_id.clone(),
        private_key: normalize_multiline_secret(read_required_secret(
            env,
            &file_config.connector.github.app.private_key_env,
            "connector.github.app.private_key_env",
        )?),
        installation_id: file_config.connector.github.app.installation_id,
        scope_org: file_config.connector.github.search_scope_org.clone(),
    })
}

fn resolve_esa_config(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
) -> Result<Option<EsaConfig>, ConfigError> {
    let Some(esa) = file_config.connector.esa.as_ref() else {
        return Ok(None);
    };

    Ok(Some(EsaConfig {
        team_name: esa.team_name.clone(),
        access_token: read_required_secret(
            env,
            &esa.access_token_env,
            "connector.esa.access_token_env",
        )?,
    }))
}

fn resolve_jira_config(
    file_config: &FileConfig,
    env: &dyn EnvironmentReader,
) -> Result<Option<JiraConfig>, ConfigError> {
    let Some(jira) = file_config.connector.jira.as_ref() else {
        return Ok(None);
    };

    Ok(Some(JiraConfig {
        site: jira.site.clone(),
        service_account_api_token: read_required_secret(
            env,
            &jira.service_account_api_token_env,
            "connector.jira.service_account_api_token_env",
        )?,
    }))
}

const DEFAULT_MEMORY_CAP: u32 = 15;

fn resolve_memory_config(file_config: &FileConfig) -> Option<SlackCanvasMemoryConfig> {
    file_config
        .memory
        .slack
        .as_ref()
        .map(|slack| SlackCanvasMemoryConfig {
            canvas_id: slack.canvas_id.clone(),
            cap: slack.cap.unwrap_or(DEFAULT_MEMORY_CAP),
        })
}

fn normalize_multiline_secret(secret: SecretString) -> SecretString {
    SecretString::new(secret.expose().replace("\\n", "\n"))
}

fn optional_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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
    use crate::config::model::{JudgeProviderConfig, LlmProviderConfig, SlackConnectionMode};

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
                (
                    "ESA_ACCESS_TOKEN".to_string(),
                    "esa-access-token".to_string(),
                ),
                (
                    "JIRA_SERVICE_ACCOUNT_API_TOKEN".to_string(),
                    "jira-service-account-token".to_string(),
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
    fn resolves_memory_config_when_absent_to_none() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert!(config.memory.is_none());
    }

    #[test]
    fn resolves_memory_slack_canvas_with_cap_default() {
        let env = FixedEnvironment::with_overrides(&[]);
        let toml = format!(
            "{}\n[memory.slack]\ncanvas_id = \"F0CANVAS\"\n",
            valid_openai_config()
        );
        let file_config = parse_runtime_config(&toml);

        let memory = resolve_app_config(file_config, &env)
            .expect("resolve config")
            .memory
            .expect("memory config present");

        assert_eq!(memory.canvas_id, "F0CANVAS");
        assert_eq!(memory.cap, 15);
    }

    #[test]
    fn resolves_memory_slack_canvas_cap_override() {
        let env = FixedEnvironment::with_overrides(&[]);
        let toml = format!(
            "{}\n[memory.slack]\ncanvas_id = \"F0CANVAS\"\ncap = 3\n",
            valid_openai_config()
        );
        let file_config = parse_runtime_config(&toml);

        let memory = resolve_app_config(file_config, &env)
            .expect("resolve config")
            .memory
            .expect("memory config present");

        assert_eq!(memory.cap, 3);
    }

    #[test]
    fn resolves_openai_backend_from_toml_and_env() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.api_key.expose(), "openai-api-key");
                assert_eq!(provider.model, "gpt-5.3-codex");
                assert_eq!(provider.reasoning_effort, "medium");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn resolves_openai_reasoning_effort_from_toml() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "model = \"gpt-5.3-codex\"\n",
            "model = \"gpt-5.3-codex\"\nreasoning_effort = \"high\"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.reasoning_effort, "high");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn passes_openai_reasoning_effort_through_without_validation() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "model = \"gpt-5.3-codex\"\n",
            "model = \"gpt-5.3-codex\"\nreasoning_effort = \"max\"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.reasoning_effort, "max");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn resolves_additional_system_prompt_when_present() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "language = \"English\"\n",
            "language = \"English\"\nadditional_system_prompt = \"  Prefer runbook links first.\\nState uncertainty explicitly.  \"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config.additional_system_prompt.as_deref(),
            Some("Prefer runbook links first.\nState uncertainty explicitly.")
        );
    }

    #[test]
    fn ignores_blank_additional_system_prompt() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "language = \"English\"\n",
            "language = \"English\"\nadditional_system_prompt = \"   \"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.additional_system_prompt, None);
    }

    #[test]
    fn accepts_legacy_additional_system_prompt_instructions_alias() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "language = \"English\"\n",
            "language = \"English\"\nadditional_system_prompt_instructions = \"Prefer runbook links first.\"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config.additional_system_prompt.as_deref(),
            Some("Prefer runbook links first.")
        );
    }

    #[test]
    fn accepts_legacy_system_prompt_instructions_alias() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "language = \"English\"\n",
            "language = \"English\"\nsystem_prompt_instructions = \"Prefer runbook links first.\"\n",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config.additional_system_prompt.as_deref(),
            Some("Prefer runbook links first.")
        );
    }

    #[test]
    fn rejects_unsupported_inactive_backend_provider_at_parse_time() {
        let toml = valid_openai_config().replace(
            "[connector.datadog]\n",
            r#"[ai.backends.unused]
provider = "unsupported-provider"

[connector.datadog]
"#,
        );

        let error = parse_file_config(Path::new(TEST_PATH), &toml)
            .expect_err("unsupported provider should fail at parse time");

        match error {
            ConfigError::ParseToml { message, .. } => {
                assert!(message.contains("unsupported-provider"), "{message}");
            }
            other => panic!("expected parse error, got {other}"),
        }
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
    fn resolves_unlisted_anthropic_model_from_toml() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_anthropic_config().replace(
            "model = \"claude-sonnet-4-6\"",
            "model = \"claude-future-model\"",
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::Anthropic(provider) => {
                assert_eq!(provider.model, "claude-future-model");
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
    fn resolves_optional_esa_connector_when_configured() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.esa]
team_name = "docs"
access_token_env = "ESA_ACCESS_TOKEN"

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let esa = config.esa.expect("esa config");

        assert_eq!(esa.team_name, "docs");
        assert_eq!(esa.access_token.expose(), "esa-access-token");
    }

    #[test]
    fn omits_esa_connector_when_not_configured() {
        let env = FixedEnvironment::with_overrides(&[("ESA_ACCESS_TOKEN", "")]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert!(config.esa.is_none());
    }

    #[test]
    fn defaults_esa_access_token_env_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.esa]
team_name = "docs"

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config.esa.expect("esa config").access_token.expose(),
            "esa-access-token"
        );
    }

    #[test]
    fn rejects_esa_connector_with_missing_access_token_env() {
        let env = FixedEnvironment::with_overrides(&[("ESA_ACCESS_TOKEN", "")]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.esa]
team_name = "docs"

[ai]
"#,
        ));

        let error = resolve_app_config(file_config, &env)
            .expect_err("missing esa access token should fail");

        match error {
            ConfigError::MissingRequiredEnv { env, field } => {
                assert_eq!(env, "ESA_ACCESS_TOKEN");
                assert_eq!(field, "connector.esa.access_token_env");
            }
            other => panic!("expected missing-env error, got {other}"),
        }
    }

    #[test]
    fn resolves_optional_jira_connector_when_configured() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.jira]
site = "acme.atlassian.net"
service_account_api_token_env = "JIRA_SERVICE_ACCOUNT_API_TOKEN"

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let jira = config.jira.expect("jira config");

        assert_eq!(jira.site, "acme.atlassian.net");
        assert_eq!(
            jira.service_account_api_token.expose(),
            "jira-service-account-token"
        );
    }

    #[test]
    fn omits_jira_connector_when_not_configured() {
        let env = FixedEnvironment::with_overrides(&[("JIRA_SERVICE_ACCOUNT_API_TOKEN", "")]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert!(config.jira.is_none());
    }

    #[test]
    fn defaults_jira_service_account_api_token_env_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.jira]
site = "acme.atlassian.net"

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(
            config
                .jira
                .expect("jira config")
                .service_account_api_token
                .expose(),
            "jira-service-account-token"
        );
    }

    #[test]
    fn rejects_jira_connector_with_missing_service_account_api_token_env() {
        let env = FixedEnvironment::with_overrides(&[("JIRA_SERVICE_ACCOUNT_API_TOKEN", "")]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[connector.jira]
site = "acme.atlassian.net"

[ai]
"#,
        ));

        let error = resolve_app_config(file_config, &env)
            .expect_err("missing jira service account api token should fail");

        match error {
            ConfigError::MissingRequiredEnv { env, field } => {
                assert_eq!(env, "JIRA_SERVICE_ACCOUNT_API_TOKEN");
                assert_eq!(field, "connector.jira.service_account_api_token_env");
            }
            other => panic!("expected missing-env error, got {other}"),
        }
    }

    #[test]
    fn defaults_server_port_at_parse_time_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config =
            parse_runtime_config(&valid_openai_config().replace("[server]\nport = 3000\n\n", ""));

        assert_eq!(file_config.server.port, 3000);

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.port, 3000);
    }

    #[test]
    fn defaults_socket_mode_to_true_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_file_config(
            Path::new(TEST_PATH),
            r#"
version = 1

[server]
port = 3000

[conversation]
language = "English"

[channel.slack]

[ai]
default_backend = "primary"

[ai.backends.primary]
provider = "openai"
model = "gpt-5.3-codex"
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
        )
        .expect("parse runtime config");

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
    fn defaults_env_reference_fields_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_openai_config()
                .replace("[channel.slack.auth]\nbot_token_env = \"SLACK_BOT_TOKEN\"\n\n", "")
                .replace("[channel.slack.socket]\napp_token_env = \"SLACK_APP_TOKEN\"\n\n", "")
                .replacen("api_key_env = \"LLM_OPENAI_API_KEY\"\n", "", 1)
                .replace(
                    "[connector.datadog]\nsite = \"datadoghq.com\"\napi_key_env = \"DATADOG_API_KEY\"\napp_key_env = \"DATADOG_APP_KEY\"\n\n",
                    "",
                )
                .replace("site = \"datadoghq.com\"\n", "")
                .replace("mcp_url = \"https://api.githubcopilot.com/mcp/\"\n", "")
                .replace("private_key_env = \"GITHUB_APP_PRIVATE_KEY\"\n", ""),
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.slack_bot_token.expose(), "xoxb-test");
        match config.slack_connection_mode {
            SlackConnectionMode::SocketMode { app_token } => {
                assert_eq!(app_token.expose(), "xapp-test-token");
            }
            SlackConnectionMode::Http => panic!("expected socket mode"),
        }
        assert_eq!(config.datadog_api_key.expose(), "dd-api-key");
        assert_eq!(config.datadog_app_key.expose(), "dd-app-key");
        assert_eq!(config.datadog_site, "datadoghq.com");
        assert_eq!(config.github.url, "https://api.githubcopilot.com/mcp/");
        assert_eq!(
            config.github.private_key.expose(),
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"
        );

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.api_key.expose(), "openai-api-key");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn defaults_http_signing_secret_env_when_section_is_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_http_config().replace(
            "[channel.slack.http]\nsigning_secret_env = \"SLACK_SIGNING_SECRET\"\n\n",
            "",
        ));

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
    fn resolves_slack_actor_authorization_from_toml_without_normalization() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[channel.slack.authorization.actors]
user_ids = ["u001", "W002", "U001"]
user_group_ids = ["s001", "S001"]
allow_bot = true

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let actors = config
            .slack_authorization
            .expect("slack authorization config")
            .actors
            .expect("actor authorization");

        assert_eq!(
            actors.user_ids,
            Some(vec![
                "u001".to_string(),
                "W002".to_string(),
                "U001".to_string()
            ])
        );
        assert_eq!(
            actors.user_group_ids,
            Some(vec!["s001".to_string(), "S001".to_string()])
        );
        assert!(actors.allow_bot);
    }

    #[test]
    fn preserves_empty_slack_authorization_arrays_as_configured_conditions() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[channel.slack.authorization.actors]
user_ids = []

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let actors = config
            .slack_authorization
            .expect("slack authorization config")
            .actors
            .expect("actor authorization");

        assert_eq!(actors.user_ids, Some(Vec::new()));
        assert_eq!(actors.user_group_ids, None);
        assert!(!actors.allow_bot);
    }

    #[test]
    fn resolves_slack_authorization_allow_bot_without_user_allowlists() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[channel.slack.authorization.actors]
allow_bot = true

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let actors = config
            .slack_authorization
            .expect("slack authorization config")
            .actors
            .expect("actor authorization");

        assert_eq!(actors.user_ids, None);
        assert_eq!(actors.user_group_ids, None);
        assert!(actors.allow_bot);
    }

    #[test]
    fn resolves_slack_channels_table_without_normalization() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = [" Alerts-PROD ", "alerts-*"]
auto_response = true
auto_response_policy = "React to incidents."

[[channel.slack.channels]]
names = ["team-sre"]

[[channel.slack.channels]]
names = ["aws-health"]
mention = false
auto_response = true
auto_response_policy = "React to AWS health notices."

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert_eq!(config.slack_channels.len(), 3);
        let first = &config.slack_channels[0];
        assert_eq!(
            first
                .names
                .iter()
                .map(|pattern| pattern.as_str().to_string())
                .collect::<Vec<_>>(),
            vec![" Alerts-PROD ".to_string(), "alerts-*".to_string()]
        );
        assert!(first.mention);
        assert!(first.auto_response);
        assert_eq!(
            first.auto_response_policy.as_deref(),
            Some("React to incidents.")
        );

        let mention_patterns = config
            .mention_channel_patterns()
            .iter()
            .map(|pattern| pattern.as_str().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            mention_patterns,
            vec![" Alerts-PROD ", "alerts-*", "team-sre"]
        );

        let auto_response_policies = config
            .auto_response_channels()
            .map(|channel| channel.auto_response_policy.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            auto_response_policies,
            vec![
                Some("React to incidents.".to_string()),
                Some("React to AWS health notices.".to_string())
            ]
        );
    }

    #[test]
    fn resolves_auto_response_channel_without_policy() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["alerts-*"]
auto_response = true

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let channel = config
            .auto_response_channels()
            .next()
            .expect("auto-response channel");

        assert_eq!(channel.auto_response_policy, None);
        assert!(config.judge_llm.is_some());
    }

    #[test]
    fn rejects_channel_entry_with_mention_and_auto_response_disabled() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["team-sre"]

[[channel.slack.channels]]
names = ["alerts-*"]
mention = false
auto_response = false

[ai]
"#,
        ));

        let error =
            resolve_app_config(file_config, &env).expect_err("meaningless entry should fail");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "channel.slack.channels[1]");
                assert!(message.contains("mention"), "{message}");
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }

    #[test]
    fn omits_judge_llm_when_no_auto_response_channels_are_configured() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["team-sre"]

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        assert!(config.judge_llm.is_none());
    }

    #[test]
    fn defaults_judge_backend_to_default_backend() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["alerts-*"]
auto_response = true
auto_response_policy = "React to incidents."

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.judge_llm.expect("judge llm config") {
            JudgeProviderConfig::OpenAi { model, api_key } => {
                assert_eq!(model, "gpt-5.3-codex");
                assert_eq!(api_key.expose(), "openai-api-key");
            }
            other => panic!("expected openai judge provider, got {other:?}"),
        }
    }

    #[test]
    fn resolves_judge_backend_with_provider_different_from_lead() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_openai_config()
                .replace(
                    "[ai]\n",
                    r#"[[channel.slack.channels]]
names = ["alerts-*"]
auto_response = true
auto_response_policy = "React to incidents."

[ai]
"#,
                )
                .replace(
                    "default_backend = \"primary\"",
                    "default_backend = \"primary\"\njudge_backend = \"fast\"",
                ),
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(_) => {}
            other => panic!("expected openai lead provider, got {other:?}"),
        }
        match config.judge_llm.expect("judge llm config") {
            JudgeProviderConfig::Anthropic { model, api_key } => {
                assert_eq!(model, "claude-haiku-4-5");
                assert_eq!(api_key.expose(), "anthropic-api-key");
            }
            other => panic!("expected anthropic judge provider, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_judge_backend() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_openai_config()
                .replace(
                    "[ai]\n",
                    r#"[[channel.slack.channels]]
names = ["alerts-*"]
auto_response = true
auto_response_policy = "React to incidents."

[ai]
"#,
                )
                .replace(
                    "default_backend = \"primary\"",
                    "default_backend = \"primary\"\njudge_backend = \"missing\"",
                ),
        );

        let error = resolve_app_config(file_config, &env).expect_err("invalid judge backend");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "ai.judge_backend");
                assert!(message.contains("unknown backend `missing`"), "{message}");
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }

    #[test]
    fn keeps_slack_authorization_actor_ids_without_format_validation() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "[ai]\n",
            r#"[channel.slack.authorization.actors]
user_ids = ["S001"]
user_group_ids = ["U001"]

[ai]
"#,
        ));

        let config = resolve_app_config(file_config, &env).expect("resolve config");
        let actors = config
            .slack_authorization
            .expect("slack authorization config")
            .actors
            .expect("actor authorization");

        assert_eq!(actors.user_ids, Some(vec!["S001".to_string()]));
        assert_eq!(actors.user_group_ids, Some(vec!["U001".to_string()]));
        assert!(!actors.allow_bot);
    }

    #[test]
    fn defaults_anthropic_api_key_env_when_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_anthropic_config().replace("api_key_env = \"LLM_ANTHROPIC_API_KEY\"\n", ""),
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::Anthropic(provider) => {
                assert_eq!(provider.api_key.expose(), "anthropic-api-key");
            }
            _ => panic!("expected anthropic provider"),
        }
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

    #[test]
    fn defaults_sub_agent_model_to_lead_model_when_overrides_omitted() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config());

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.model, "gpt-5.3-codex");
                assert_eq!(provider.sub_agent_model, "gpt-5.3-codex");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn resolves_distinct_lead_and_sub_agent_models_within_same_provider() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(
            &valid_openai_config()
                .replace(
                    r#"[ai.backends.fast]
provider = "anthropic"
model = "claude-haiku-4-5"
api_key_env = "LLM_ANTHROPIC_API_KEY"
"#,
                    r#"[ai.backends.fast]
provider = "openai"
model = "gpt-5.3-mini"
api_key_env = "LLM_OPENAI_API_KEY"
"#,
                )
                .replace(
                    "default_backend = \"primary\"",
                    "default_backend = \"primary\"\nlead_backend = \"primary\"\nsub_agent_backend = \"fast\"",
                ),
        );

        let config = resolve_app_config(file_config, &env).expect("resolve config");

        match config.llm.provider {
            LlmProviderConfig::OpenAi(provider) => {
                assert_eq!(provider.model, "gpt-5.3-codex");
                assert_eq!(provider.sub_agent_model, "gpt-5.3-mini");
            }
            _ => panic!("expected openai provider"),
        }
    }

    #[test]
    fn rejects_lead_and_sub_agent_backends_with_different_providers() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "default_backend = \"primary\"",
            "default_backend = \"primary\"\nlead_backend = \"primary\"\nsub_agent_backend = \"fast\"",
        ));

        let error = resolve_app_config(file_config, &env).expect_err("provider mismatch");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "ai.sub_agent_backend");
                assert!(message.contains("provider `anthropic`"), "{message}");
                assert!(message.contains("provider `openai`"), "{message}");
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }

    #[test]
    fn rejects_unknown_sub_agent_backend() {
        let env = FixedEnvironment::with_overrides(&[]);
        let file_config = parse_runtime_config(&valid_openai_config().replace(
            "default_backend = \"primary\"",
            "default_backend = \"primary\"\nsub_agent_backend = \"missing\"",
        ));

        let error = resolve_app_config(file_config, &env).expect_err("invalid sub-agent backend");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "ai.sub_agent_backend");
                assert!(message.contains("unknown backend `missing`"), "{message}");
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
socket_mode = true

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
model = "gpt-5.3-codex"
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
        valid_openai_config().replace("socket_mode = true", "socket_mode = false")
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

    fn base_config_with_ai_block(ai_block: &str) -> String {
        format!(
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

{ai_block}
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
        )
    }

    fn valid_bedrock_config() -> String {
        base_config_with_ai_block(
            r#"[ai]
default_backend = "bedrock"

[ai.backends.bedrock]
provider = "bedrock"
model_id = "anthropic.claude-3-7-sonnet-20250219-v1:0"
aws_profile = "prod-sso"
aws_region = "ap-northeast-1"

"#,
        )
    }

    fn valid_vertex_ai_config() -> String {
        base_config_with_ai_block(
            r#"[ai]
default_backend = "vertex"

[ai.backends.vertex]
provider = "vertexai"
project_id = "example-project"
location = "global"
model_id = "gemini-2.5-pro"

"#,
        )
    }
}
