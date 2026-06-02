use std::collections::BTreeMap;
use std::path::Path;

use serde::de::{Error as DeError, Unexpected};
use serde::{Deserialize, Deserializer};

use super::ConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct FileConfig {
    pub version: u32,
    #[serde(default)]
    pub server: ServerFileConfig,
    pub conversation: ConversationFileConfig,
    pub channel: ChannelFileConfig,
    pub ai: AiFileConfig,
    pub connector: ConnectorFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct ServerFileConfig {
    #[serde(default = "default_server_port", deserialize_with = "require_port")]
    pub port: u16,
}

impl Default for ServerFileConfig {
    fn default() -> Self {
        Self {
            port: default_server_port(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct ConversationFileConfig {
    #[serde(
        default = "default_conversation_language",
        deserialize_with = "require_non_empty_string"
    )]
    pub language: String,
    #[serde(
        alias = "additional_system_prompt_instructions",
        alias = "system_prompt_instructions"
    )]
    pub additional_system_prompt: Option<String>,
}

impl Default for ConversationFileConfig {
    fn default() -> Self {
        Self {
            language: default_conversation_language(),
            additional_system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ChannelFileConfig {
    pub slack: SlackFileConfig,
}

fn default_socket_mode() -> bool {
    true
}

fn default_server_port() -> u16 {
    3000
}

fn default_conversation_language() -> String {
    "English".to_string()
}

fn default_slack_bot_token_env() -> String {
    "SLACK_BOT_TOKEN".to_string()
}

fn default_slack_app_token_env() -> String {
    "SLACK_APP_TOKEN".to_string()
}

fn default_slack_signing_secret_env() -> String {
    "SLACK_SIGNING_SECRET".to_string()
}

fn default_datadog_site() -> String {
    "datadoghq.com".to_string()
}

fn default_datadog_api_key_env() -> String {
    "DATADOG_API_KEY".to_string()
}

fn default_datadog_app_key_env() -> String {
    "DATADOG_APP_KEY".to_string()
}

fn default_github_mcp_url() -> String {
    "https://api.githubcopilot.com/mcp/".to_string()
}

fn default_github_app_private_key_env() -> String {
    "GITHUB_APP_PRIVATE_KEY".to_string()
}

fn default_esa_access_token_env() -> String {
    "ESA_ACCESS_TOKEN".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackFileConfig {
    #[serde(default = "default_socket_mode")]
    pub socket_mode: bool,
    #[serde(default)]
    pub auth: SlackAuthFileConfig,
    pub authorization: Option<SlackAuthorizationFileConfig>,
    #[serde(default)]
    pub socket: SlackSocketFileConfig,
    #[serde(default)]
    pub http: SlackHttpFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SlackAuthFileConfig {
    #[serde(
        default = "default_slack_bot_token_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub bot_token_env: String,
}

impl Default for SlackAuthFileConfig {
    fn default() -> Self {
        Self {
            bot_token_env: default_slack_bot_token_env(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SlackSocketFileConfig {
    #[serde(
        default = "default_slack_app_token_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub app_token_env: String,
}

impl Default for SlackSocketFileConfig {
    fn default() -> Self {
        Self {
            app_token_env: default_slack_app_token_env(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SlackHttpFileConfig {
    #[serde(
        default = "default_slack_signing_secret_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub signing_secret_env: String,
}

impl Default for SlackHttpFileConfig {
    fn default() -> Self {
        Self {
            signing_secret_env: default_slack_signing_secret_env(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackAuthorizationFileConfig {
    pub channels: SlackAuthorizationChannelsFileConfig,
    pub actors: Option<SlackAuthorizationActorsFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackAuthorizationChannelsFileConfig {
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SlackAuthorizationActorsFileConfig {
    pub user_ids: Option<Vec<String>>,
    pub user_group_ids: Option<Vec<String>>,
    #[serde(default)]
    pub allow_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AiFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub default_backend: String,
    pub backends: BTreeMap<String, AiBackendFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "provider", deny_unknown_fields)]
pub(crate) enum AiBackendFileConfig {
    #[serde(rename = "openai")]
    OpenAi {
        #[serde(deserialize_with = "require_non_empty_string")]
        model: String,
        #[serde(default, deserialize_with = "optional_non_empty_string")]
        api_key_env: Option<String>,
        #[serde(default, deserialize_with = "optional_non_empty_string")]
        reasoning_effort: Option<String>,
    },
    #[serde(rename = "anthropic")]
    Anthropic {
        #[serde(deserialize_with = "require_non_empty_string")]
        model: String,
        #[serde(default, deserialize_with = "optional_non_empty_string")]
        api_key_env: Option<String>,
    },
    #[serde(rename = "bedrock")]
    Bedrock {
        #[serde(deserialize_with = "require_non_empty_string")]
        model_id: String,
        aws_profile: Option<String>,
        aws_region: Option<String>,
    },
    #[serde(rename = "vertexai", alias = "vertex_ai")]
    VertexAi {
        #[serde(deserialize_with = "require_non_empty_string")]
        project_id: String,
        #[serde(deserialize_with = "require_non_empty_string")]
        location: String,
        #[serde(deserialize_with = "require_non_empty_string")]
        model_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ConnectorFileConfig {
    #[serde(default)]
    pub datadog: DatadogConnectorFileConfig,
    pub github: GitHubConnectorFileConfig,
    pub esa: Option<EsaConnectorFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct DatadogConnectorFileConfig {
    #[serde(
        default = "default_datadog_site",
        deserialize_with = "require_non_empty_string"
    )]
    pub site: String,
    #[serde(
        default = "default_datadog_api_key_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub api_key_env: String,
    #[serde(
        default = "default_datadog_app_key_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub app_key_env: String,
}

impl Default for DatadogConnectorFileConfig {
    fn default() -> Self {
        Self {
            site: default_datadog_site(),
            api_key_env: default_datadog_api_key_env(),
            app_key_env: default_datadog_app_key_env(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubConnectorFileConfig {
    #[serde(
        default = "default_github_mcp_url",
        deserialize_with = "require_non_empty_string"
    )]
    pub mcp_url: String,
    #[serde(deserialize_with = "require_non_empty_string")]
    pub search_scope_org: String,
    pub app: GitHubAppFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubAppFileConfig {
    #[serde(deserialize_with = "require_positive_u64_string")]
    pub app_id: String,
    #[serde(deserialize_with = "require_positive_u32")]
    pub installation_id: u32,
    #[serde(
        default = "default_github_app_private_key_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub private_key_env: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct EsaConnectorFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub team_name: String,
    #[serde(
        default = "default_esa_access_token_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub access_token_env: String,
}

pub(crate) fn parse_file_config(path: &Path, contents: &str) -> Result<FileConfig, ConfigError> {
    toml::from_str(contents).map_err(|error| ConfigError::ParseToml {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn require_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    non_empty_string(String::deserialize(deserializer)?)
}

fn optional_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(non_empty_string)
        .transpose()
}

fn non_empty_string<E>(value: String) -> Result<String, E>
where
    E: DeError,
{
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(E::invalid_value(
            Unexpected::Str(&value),
            &"a non-empty string",
        ));
    }

    Ok(trimmed.to_string())
}

fn require_port<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    if value == 0 || value > 65_535 {
        return Err(D::Error::invalid_value(
            Unexpected::Unsigned(u64::from(value)),
            &"a positive integer between 1 and 65535",
        ));
    }

    Ok(value as u16)
}

fn parse_positive_integer<T>(value: &str) -> Option<T>
where
    T: std::str::FromStr + PartialOrd + Default,
{
    value.trim().parse::<T>().ok().filter(|n| *n > T::default())
}

fn require_positive_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse_positive_integer::<u32>(&value).ok_or_else(|| {
        D::Error::invalid_value(
            Unexpected::Str(&value),
            &"a string containing a positive integer",
        )
    })
}

fn require_positive_u64_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let trimmed = value.trim();
    parse_positive_integer::<u64>(trimmed)
        .map(|_| trimmed.to_string())
        .ok_or_else(|| {
            D::Error::invalid_value(
                Unexpected::Str(&value),
                &"a string containing a positive integer",
            )
        })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::config::ConfigError;

    use super::{AiBackendFileConfig, parse_file_config};

    const TEST_PATH: &str = "/test/reili.toml";

    #[test]
    fn rejects_empty_required_string_at_parse_time() {
        let toml = valid_config().replace("language = \"English\"", "language = \"   \"");

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
    }

    #[test]
    fn rejects_empty_optional_string_at_parse_time_when_present() {
        let toml = valid_config().replace(
            "model = \"gpt-5.4\"",
            "model = \"gpt-5.4\"\napi_key_env = \"\"",
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
    }

    #[test]
    fn parses_ai_backend_as_provider_specific_variant() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        match config.ai.backends.get("primary").expect("primary backend") {
            AiBackendFileConfig::OpenAi {
                model,
                api_key_env,
                reasoning_effort,
            } => {
                assert_eq!(model, "gpt-5.4");
                assert_eq!(api_key_env, &None);
                assert_eq!(reasoning_effort, &None);
            }
            other => panic!("expected OpenAI backend, got {other:?}"),
        }
    }

    #[test]
    fn rejects_ai_backend_missing_required_field_at_parse_time() {
        let toml = valid_config().replace("model = \"gpt-5.4\"\n", "");

        let message = parse_error_message(&toml);

        assert!(message.contains("missing field `model`"), "{message}");
    }

    #[test]
    fn rejects_provider_specific_unknown_field_at_parse_time() {
        let toml = valid_config().replace(
            "model = \"gpt-5.4\"\n",
            "model = \"gpt-5.4\"\nmodel_id = \"gemini-2.5-flash\"\n",
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("unknown field `model_id`"), "{message}");
    }

    #[test]
    fn accepts_vertex_ai_provider_alias_at_parse_time() {
        let toml = valid_config().replace(
            r#"[ai.backends.primary]
provider = "openai"
model = "gpt-5.4"
"#,
            r#"[ai.backends.primary]
provider = "vertex_ai"
project_id = "example-project"
location = "global"
model_id = "gemini-2.5-flash"
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");

        match config.ai.backends.get("primary").expect("primary backend") {
            AiBackendFileConfig::VertexAi {
                project_id,
                location,
                model_id,
            } => {
                assert_eq!(project_id, "example-project");
                assert_eq!(location, "global");
                assert_eq!(model_id, "gemini-2.5-flash");
            }
            other => panic!("expected Vertex AI backend, got {other:?}"),
        }
    }

    #[test]
    fn defaults_slack_connection_env_references_at_parse_time() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        assert_eq!(config.channel.slack.socket.app_token_env, "SLACK_APP_TOKEN");
        assert_eq!(
            config.channel.slack.http.signing_secret_env,
            "SLACK_SIGNING_SECRET"
        );
    }

    #[test]
    fn rejects_empty_slack_connection_env_references_at_parse_time() {
        for connection_config in [
            r#"[channel.slack.socket]
app_token_env = " "

"#,
            r#"[channel.slack.http]
signing_secret_env = ""

"#,
        ] {
            let toml = valid_config().replace("[ai]\n", &format!("{connection_config}[ai]\n"));

            let message = parse_error_message(&toml);

            assert!(message.contains("non-empty string"), "{message}");
        }
    }

    #[test]
    fn rejects_invalid_server_port_at_parse_time() {
        for invalid_port in ["0", "65536"] {
            let toml = valid_config().replace("port = 3000", &format!("port = {invalid_port}"));

            let message = parse_error_message(&toml);

            assert!(
                message.contains("positive integer between 1 and 65535"),
                "{message}"
            );
        }
    }

    #[test]
    fn rejects_invalid_github_id_at_parse_time() {
        for (field, replacement) in [
            ("app_id = \"12345\"", "app_id = \"0\""),
            (
                "installation_id = \"67890\"",
                "installation_id = \"not-a-number\"",
            ),
        ] {
            let toml = valid_config().replace(field, replacement);

            let message = parse_error_message(&toml);

            assert!(message.contains("positive integer"), "{message}");
        }
    }

    #[test]
    fn rejects_slack_authorization_without_channel_names_at_parse_time() {
        for (authorization_config, missing_field) in [
            (
                r#"[channel.slack.authorization]

"#,
                "channels",
            ),
            (
                r#"[channel.slack.authorization.actors]
user_ids = ["U001"]

"#,
                "channels",
            ),
            (
                r#"[channel.slack.authorization.channels]

"#,
                "names",
            ),
        ] {
            let toml = valid_config().replace("[ai]\n", &format!("{authorization_config}[ai]\n"));

            let message = parse_error_message(&toml);

            assert!(
                message.contains(&format!("missing field `{missing_field}`")),
                "{message}"
            );
        }
    }

    fn parse_error_message(toml: &str) -> String {
        match parse_file_config(Path::new(TEST_PATH), toml).expect_err("parse should fail") {
            ConfigError::ParseToml { path, message } => {
                assert_eq!(path, PathBuf::from(TEST_PATH));
                message
            }
            other => panic!("expected parse error, got {other}"),
        }
    }

    fn valid_config() -> String {
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
model = "gpt-5.4"

[connector.github]
search_scope_org = "example-org"

[connector.github.app]
app_id = "12345"
installation_id = "67890"
"#
        .to_string()
    }
}
