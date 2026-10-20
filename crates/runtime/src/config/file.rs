use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;

use serde::de::{Error as DeError, MapAccess, Unexpected, Visitor};
use serde::{Deserialize, Deserializer};

use super::ConfigError;
use super::objective_file::{ObjectiveFileConfig, WorkStateFileConfig};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct FileConfig {
    pub version: u32,
    #[serde(default)]
    pub server: ServerFileConfig,
    pub conversation: ConversationFileConfig,
    pub channel: ChannelFileConfig,
    pub ai: AiFileConfig,
    pub connector: ConnectorFileConfig,
    #[serde(default)]
    pub memory: MemoryFileConfig,
    #[serde(default)]
    pub tracing: TracingFileConfig,
    #[serde(default)]
    pub work_state: WorkStateFileConfig,
    #[serde(default)]
    pub objectives: Vec<ObjectiveFileConfig>,
}

/// Optional top-level `[memory]` block. Memory is a cross-channel concern, so it lives at the top
/// level rather than under `[channel]`. Backends are keyed by sub-table (currently only `slack`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MemoryFileConfig {
    pub slack: Option<MemorySlackFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemorySlackFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub canvas_id: String,
    pub cap: Option<u32>,
}

/// Optional top-level `[tracing]` block. Tracing export is a cross-cutting concern independent of
/// any single connector, so it lives at the top level like `[memory]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TracingFileConfig {
    pub otlp: Option<TracingOtlpFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TracingOtlpFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub endpoint: String,
    pub service_name: Option<String>,
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

fn default_jira_service_account_api_token_env() -> String {
    "JIRA_SERVICE_ACCOUNT_API_TOKEN".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackFileConfig {
    #[serde(default = "default_socket_mode")]
    pub socket_mode: bool,
    #[serde(default)]
    pub auth: SlackAuthFileConfig,
    pub authorization: Option<SlackAuthorizationFileConfig>,
    #[serde(default)]
    pub channels: Vec<SlackChannelFileConfig>,
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
#[serde(deny_unknown_fields)]
pub(crate) struct SlackAuthorizationFileConfig {
    pub actors: Option<SlackAuthorizationActorsFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackChannelFileConfig {
    pub names: Vec<String>,
    #[serde(default = "default_true")]
    pub mention: bool,
    #[serde(default)]
    pub auto_response: bool,
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub auto_response_policy: Option<String>,
}

fn default_true() -> bool {
    true
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
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub lead_backend: Option<String>,
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub sub_agent_backend: Option<String>,
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub judge_backend: Option<String>,
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub web_search_backend: Option<String>,
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
        aws_assume_role_arn: Option<String>,
    },
    #[serde(rename = "bedrock_mantle")]
    BedrockMantle {
        #[serde(deserialize_with = "require_non_empty_string")]
        model_id: String,
        #[serde(deserialize_with = "require_non_empty_string")]
        aws_region: String,
        #[serde(default, deserialize_with = "optional_non_empty_string")]
        api_key_env: Option<String>,
        aws_profile: Option<String>,
        aws_assume_role_arn: Option<String>,
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
    pub jira: Option<JiraConnectorFileConfig>,
}

/// `[connector.datadog]`. Datadog is always on: without a `[connector.datadog.connection.<name>]`
/// table, a connection with every default is used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct DatadogConnectorFileConfig {
    /// Name of the `[connector.datadog.connection.<name>]` the Datadog MCP connector uses. Optional
    /// only while at most one connection is defined.
    #[serde(deserialize_with = "optional_non_empty_string")]
    pub mcp_connection: Option<String>,
    /// Datadog connections keyed by the name other settings use to select them.
    #[serde(deserialize_with = "deserialize_named_datadog_connections")]
    pub connection: BTreeMap<String, DatadogConnectionFileConfig>,
    #[serde(flatten, deserialize_with = "reject_unnamed_datadog_settings")]
    _unnamed_settings: (),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct DatadogConnectionFileConfig {
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

impl Default for DatadogConnectionFileConfig {
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
    /// Name of the `[connector.github.connection.<name>]` the MCP connector authenticates with. Optional
    /// only while a single connection is defined.
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub mcp_connection: Option<String>,
    /// GitHub connections keyed by the name other settings use to select them.
    #[serde(default, deserialize_with = "deserialize_named_github_connections")]
    pub connection: BTreeMap<String, GitHubConnectionFileConfig>,
    #[serde(flatten, deserialize_with = "reject_unnamed_github_settings")]
    _unnamed_settings: (),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubConnectionFileConfig {
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

/// Optional `[connector.esa]` block. esa stays disabled until a connection is defined.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct EsaConnectorFileConfig {
    /// Name of the `[connector.esa.connection.<name>]` the esa sub-agent uses. Optional only
    /// while a single connection is defined.
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub agent_connection: Option<String>,
    /// esa teams keyed by the name other settings use to select them.
    #[serde(default, deserialize_with = "deserialize_named_esa_connections")]
    pub connection: BTreeMap<String, EsaConnectionFileConfig>,
    #[serde(flatten, deserialize_with = "reject_unnamed_esa_settings")]
    _unnamed_settings: (),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct EsaConnectionFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub team_name: String,
    #[serde(
        default = "default_esa_access_token_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub access_token_env: String,
}

/// Optional `[connector.jira]` block. JIRA stays disabled until a connection is defined.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct JiraConnectorFileConfig {
    /// Name of the `[connector.jira.connection.<name>]` the investigation sub-agent uses. Optional
    /// only while a single connection is defined.
    #[serde(default, deserialize_with = "optional_non_empty_string")]
    pub mcp_connection: Option<String>,
    /// Rovo MCP connections keyed by the name other settings use to select them.
    #[serde(default, deserialize_with = "deserialize_named_jira_connections")]
    pub connection: BTreeMap<String, JiraConnectionFileConfig>,
    #[serde(flatten, deserialize_with = "reject_unnamed_jira_settings")]
    _unnamed_settings: (),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct JiraConnectionFileConfig {
    #[serde(deserialize_with = "require_non_empty_string")]
    pub site: String,
    #[serde(
        default = "default_jira_service_account_api_token_env",
        deserialize_with = "require_non_empty_string"
    )]
    pub service_account_api_token_env: String,
}

pub(crate) fn parse_file_config(path: &Path, contents: &str) -> Result<FileConfig, ConfigError> {
    toml::from_str(contents).map_err(|error| ConfigError::ParseToml {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// How a table of named connector instances reads in error messages.
struct NamedTablesSpec {
    /// What one instance is called, e.g. `GitHub connection`.
    kind: &'static str,
    /// The table holding the instances, e.g. `[connector.github.connection]`.
    parent: &'static str,
    /// A sample instance table, e.g. `[connector.github.connection.default]`.
    example: &'static str,
    /// Keys that used to sit directly in `parent`, before instances became named tables.
    unnamed_keys: &'static [&'static str],
    /// Whether an empty `parent` is an error.
    require_one: bool,
}

fn deserialize_named_github_connections<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, GitHubConnectionFileConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_named_tables(
        deserializer,
        NamedTablesSpec {
            kind: "GitHub connection",
            parent: "[connector.github.connection]",
            example: "[connector.github.connection.default]",
            unnamed_keys: &["app_id", "installation_id", "private_key_env"],
            require_one: true,
        },
    )
}

fn deserialize_named_jira_connections<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, JiraConnectionFileConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_named_tables(
        deserializer,
        NamedTablesSpec {
            kind: "Jira connection",
            parent: "[connector.jira.connection]",
            example: "[connector.jira.connection.default]",
            unnamed_keys: &[],
            require_one: false,
        },
    )
}

fn deserialize_named_tables<'de, D, T>(
    deserializer: D,
    spec: NamedTablesSpec,
) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct NamedTablesVisitor<T> {
        spec: NamedTablesSpec,
        instances: PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for NamedTablesVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = BTreeMap<String, T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(
                formatter,
                "{}s as named tables such as `{}`",
                self.spec.kind, self.spec.example
            )
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let spec = &self.spec;
            let mut instances = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if spec.unnamed_keys.contains(&key.as_str()) {
                    return Err(A::Error::custom(format!(
                        "`{key}` must sit in a named table such as `{}`, not directly under `{}`",
                        spec.example, spec.parent
                    )));
                }
                let name = non_empty_string::<A::Error>(key)?;
                let instance = map.next_value::<T>()?;
                if instances.insert(name.clone(), instance).is_some() {
                    return Err(A::Error::custom(format!(
                        "duplicate {} name `{name}`",
                        spec.kind
                    )));
                }
            }
            if spec.require_one && instances.is_empty() {
                return Err(A::Error::custom(format!(
                    "define at least one {}, such as `{}`",
                    spec.kind, spec.example
                )));
            }
            Ok(instances)
        }
    }

    deserializer.deserialize_map(NamedTablesVisitor {
        spec,
        instances: PhantomData,
    })
}

fn deserialize_named_datadog_connections<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, DatadogConnectionFileConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_named_tables(
        deserializer,
        NamedTablesSpec {
            kind: "Datadog connection",
            parent: "[connector.datadog.connection]",
            example: "[connector.datadog.connection.default]",
            unnamed_keys: &[],
            require_one: false,
        },
    )
}

fn deserialize_named_esa_connections<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, EsaConnectionFileConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_named_tables(
        deserializer,
        NamedTablesSpec {
            kind: "esa connection",
            parent: "[connector.esa.connection]",
            example: "[connector.esa.connection.default]",
            unnamed_keys: &[],
            require_one: false,
        },
    )
}

/// A connector table whose settings moved into named tables. It collects every key that is not a
/// field of the table itself, so a leftover from the unnamed layout fails with a migration hint
/// instead of being ignored (which would silently fall back to defaults or disable the connector).
struct UnnamedSettingsSpec {
    /// The connector table, e.g. `[connector.jira]`.
    parent: &'static str,
    /// A sample connection table, e.g. `[connector.jira.connection.default]`.
    example: &'static str,
    /// Keys that used to sit directly in `parent`.
    unnamed_keys: &'static [&'static str],
    /// Keys of `parent` that were renamed, as (old, new) pairs.
    renamed_keys: &'static [(&'static str, &'static str)],
    /// Fields `parent` itself accepts, reported for any other unknown key.
    fields: &'static [&'static str],
}

fn reject_unnamed_github_settings<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    reject_unnamed_settings(
        deserializer,
        &UnnamedSettingsSpec {
            parent: "[connector.github]",
            example: "[connector.github.connection.default]",
            unnamed_keys: &[],
            renamed_keys: &[("app", "connection")],
            fields: &[
                "mcp_url",
                "search_scope_org",
                "mcp_connection",
                "connection",
            ],
        },
    )
}

fn reject_unnamed_datadog_settings<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    reject_unnamed_settings(
        deserializer,
        &UnnamedSettingsSpec {
            parent: "[connector.datadog]",
            example: "[connector.datadog.connection.default]",
            unnamed_keys: &["site", "api_key_env", "app_key_env"],
            renamed_keys: &[],
            fields: &["mcp_connection", "connection"],
        },
    )
}

fn reject_unnamed_esa_settings<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    reject_unnamed_settings(
        deserializer,
        &UnnamedSettingsSpec {
            parent: "[connector.esa]",
            example: "[connector.esa.connection.default]",
            unnamed_keys: &["team_name", "access_token_env"],
            renamed_keys: &[],
            fields: &["agent_connection", "connection"],
        },
    )
}

fn reject_unnamed_jira_settings<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    reject_unnamed_settings(
        deserializer,
        &UnnamedSettingsSpec {
            parent: "[connector.jira]",
            example: "[connector.jira.connection.default]",
            unnamed_keys: &["site", "service_account_api_token_env"],
            renamed_keys: &[],
            fields: &["mcp_connection", "connection"],
        },
    )
}

fn reject_unnamed_settings<'de, D>(
    deserializer: D,
    spec: &UnnamedSettingsSpec,
) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    struct RejectUnnamedSettingsVisitor<'a>(&'a UnnamedSettingsSpec);

    impl<'de> Visitor<'de> for RejectUnnamedSettingsVisitor<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "the fields of `{}`", self.0.parent)
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let Some(key) = map.next_key::<String>()? else {
                return Ok(());
            };
            if let Some((_, new)) = self.0.renamed_keys.iter().find(|(old, _)| *old == key) {
                return Err(A::Error::custom(format!(
                    "`{key}` was renamed to `{new}`: define a named table such as `{}`",
                    self.0.example
                )));
            }
            if self.0.unnamed_keys.contains(&key.as_str()) {
                return Err(A::Error::custom(format!(
                    "`{key}` must sit in a named table such as `{}`, not directly under `{}`",
                    self.0.example, self.0.parent
                )));
            }
            Err(A::Error::unknown_field(&key, self.0.fields))
        }
    }

    deserializer.deserialize_map(RejectUnnamedSettingsVisitor(spec))
}

fn require_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    non_empty_string(String::deserialize(deserializer)?)
}

pub(super) fn optional_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
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
    fn parses_bedrock_mantle_backend_with_api_key_at_parse_time() {
        let toml = valid_config().replace(
            r#"[ai.backends.primary]
provider = "openai"
model = "gpt-5.4"
"#,
            r#"[ai.backends.primary]
provider = "bedrock_mantle"
model_id = "openai.gpt-5.6-sol"
aws_region = "us-east-1"
api_key_env = "LLM_BEDROCK_MANTLE_API_KEY"
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");

        match config.ai.backends.get("primary").expect("primary backend") {
            AiBackendFileConfig::BedrockMantle {
                model_id,
                aws_region,
                api_key_env,
                aws_profile,
                aws_assume_role_arn,
            } => {
                assert_eq!(model_id, "openai.gpt-5.6-sol");
                assert_eq!(aws_region, "us-east-1");
                assert_eq!(api_key_env.as_deref(), Some("LLM_BEDROCK_MANTLE_API_KEY"));
                assert_eq!(aws_profile, &None);
                assert_eq!(aws_assume_role_arn, &None);
            }
            other => panic!("expected bedrock_mantle backend, got {other:?}"),
        }
    }

    #[test]
    fn parses_bedrock_mantle_backend_with_iam_role_at_parse_time() {
        let toml = valid_config().replace(
            r#"[ai.backends.primary]
provider = "openai"
model = "gpt-5.4"
"#,
            r#"[ai.backends.primary]
provider = "bedrock_mantle"
model_id = "openai.gpt-5.6-sol"
aws_region = "us-east-1"
aws_profile = "prod-sso"
aws_assume_role_arn = "arn:aws:iam::111111111111:role/ReiliMantle"
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");

        match config.ai.backends.get("primary").expect("primary backend") {
            AiBackendFileConfig::BedrockMantle {
                api_key_env,
                aws_profile,
                aws_assume_role_arn,
                ..
            } => {
                assert_eq!(api_key_env, &None);
                assert_eq!(aws_profile.as_deref(), Some("prod-sso"));
                assert_eq!(
                    aws_assume_role_arn.as_deref(),
                    Some("arn:aws:iam::111111111111:role/ReiliMantle")
                );
            }
            other => panic!("expected bedrock_mantle backend, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bedrock_mantle_backend_missing_aws_region_at_parse_time() {
        let toml = valid_config().replace(
            r#"[ai.backends.primary]
provider = "openai"
model = "gpt-5.4"
"#,
            r#"[ai.backends.primary]
provider = "bedrock_mantle"
model_id = "openai.gpt-5.6-sol"
"#,
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("missing field `aws_region`"), "{message}");
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
    fn parses_several_named_github_connections_and_the_mcp_connection_selection() {
        let toml = valid_config()
            .replace(
                "search_scope_org = \"example-org\"\n",
                "search_scope_org = \"example-org\"\nmcp_connection = \"reader\"\n",
            )
            .replace(
                "[connector.github.connection.default]",
                "[connector.github.connection.reader]",
            )
            + r#"
[connector.github.connection.work_state]
app_id = "99999"
installation_id = "4242"
private_key_env = "WORK_STATE_APP_PRIVATE_KEY"
"#;

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let github = &config.connector.github;

        assert_eq!(github.mcp_connection.as_deref(), Some("reader"));
        assert_eq!(
            github
                .connection
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["reader", "work_state"]
        );
        assert_eq!(github.connection["work_state"].app_id, "99999");
        assert_eq!(github.connection["work_state"].installation_id, 4242);
        assert_eq!(
            github.connection["work_state"].private_key_env,
            "WORK_STATE_APP_PRIVATE_KEY"
        );
        assert_eq!(
            github.connection["reader"].private_key_env,
            "GITHUB_APP_PRIVATE_KEY"
        );
    }

    #[test]
    fn rejects_github_settings_outside_a_named_table_at_parse_time() {
        let toml = valid_config().replace(
            "[connector.github.connection.default]\n",
            "[connector.github.connection]\n",
        );

        let message = parse_error_message(&toml);

        assert!(
            message.contains("`app_id` must sit in a named table"),
            "{message}"
        );
    }

    #[test]
    fn rejects_the_old_github_app_table_with_a_migration_hint_at_parse_time() {
        let toml = valid_config().replace(
            "[connector.github.connection.default]\n",
            "[connector.github.app]\n",
        );

        let message = parse_error_message(&toml);

        assert!(
            message.contains(
                "`app` was renamed to `connection`: define a named table such as `[connector.github.connection.default]`"
            ),
            "{message}"
        );
    }

    #[test]
    fn rejects_unknown_github_connector_keys_at_parse_time() {
        let toml = valid_config().replace(
            "search_scope_org = \"example-org\"\n",
            "search_scope_org = \"example-org\"\nmcp_conection = \"reader\"\n",
        );

        let message = parse_error_message(&toml);

        assert!(
            message.contains("unknown field `mcp_conection`"),
            "{message}"
        );
    }

    #[test]
    fn rejects_a_github_connection_table_without_any_named_connection_at_parse_time() {
        let toml = valid_config().replace(
            "[connector.github.connection.default]\napp_id = \"12345\"\ninstallation_id = \"67890\"\n",
            "[connector.github.connection]\n",
        );

        let message = parse_error_message(&toml);

        assert!(
            message.contains("define at least one GitHub connection"),
            "{message}"
        );
    }

    #[test]
    fn rejects_an_empty_github_connection_name_at_parse_time() {
        let toml = valid_config().replace(
            "[connector.github.connection.default]",
            "[connector.github.connection.\" \"]",
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
    }

    #[test]
    fn jira_connector_is_optional_and_has_no_connections_by_default() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");
        assert!(config.connector.jira.is_none());

        let toml = valid_config() + "\n[connector.jira]\n";
        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let jira = config.connector.jira.expect("connector.jira present");

        assert!(jira.connection.is_empty());
        assert_eq!(jira.mcp_connection, None);
    }

    #[test]
    fn parses_named_jira_connections_and_the_mcp_connection_selection() {
        let toml = valid_config()
            + r#"
[connector.jira]
mcp_connection = "reader"

[connector.jira.connection.reader]
site = "acme.atlassian.net"

[connector.jira.connection.work_state]
site = "work.atlassian.net"
service_account_api_token_env = "JIRA_WORK_STATE_API_TOKEN"
"#;

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let jira = config.connector.jira.expect("connector.jira present");

        assert_eq!(jira.mcp_connection.as_deref(), Some("reader"));
        assert_eq!(
            jira.connection
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["reader", "work_state"]
        );
        assert_eq!(
            jira.connection["reader"].service_account_api_token_env,
            "JIRA_SERVICE_ACCOUNT_API_TOKEN"
        );
        assert_eq!(jira.connection["work_state"].site, "work.atlassian.net");
        assert_eq!(
            jira.connection["work_state"].service_account_api_token_env,
            "JIRA_WORK_STATE_API_TOKEN"
        );
    }

    #[test]
    fn rejects_jira_settings_outside_a_named_table_at_parse_time() {
        for setting in [
            "site = \"acme.atlassian.net\"",
            "service_account_api_token_env = \"JIRA_SERVICE_ACCOUNT_API_TOKEN\"",
        ] {
            let toml = valid_config() + &format!("\n[connector.jira]\n{setting}\n");

            let message = parse_error_message(&toml);

            assert!(
                message.contains(
                    "must sit in a named table such as `[connector.jira.connection.default]`"
                ),
                "{setting}: {message}"
            );
        }
    }

    #[test]
    fn rejects_unknown_jira_connector_keys_at_parse_time() {
        let toml = valid_config() + "\n[connector.jira]\nmcp_acount = \"reader\"\n";

        let message = parse_error_message(&toml);

        assert!(message.contains("unknown field `mcp_acount`"), "{message}");
    }

    #[test]
    fn rejects_a_jira_connection_without_a_site_at_parse_time() {
        let toml = valid_config()
            + "\n[connector.jira.connection.default]\nservice_account_api_token_env = \"JIRA_TOKEN\"\n";

        let message = parse_error_message(&toml);

        assert!(message.contains("missing field `site`"), "{message}");
    }

    #[test]
    fn parses_named_datadog_connections_with_per_connection_defaults() {
        let toml = valid_config()
            + r#"
[connector.datadog]
mcp_connection = "reader"

[connector.datadog.connection.reader]

[connector.datadog.connection.eu]
site = "datadoghq.eu"
api_key_env = "DATADOG_EU_API_KEY"
"#;

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let datadog = &config.connector.datadog;

        assert_eq!(datadog.mcp_connection.as_deref(), Some("reader"));
        assert_eq!(datadog.connection["reader"].site, "datadoghq.com");
        assert_eq!(datadog.connection["reader"].api_key_env, "DATADOG_API_KEY");
        assert_eq!(datadog.connection["reader"].app_key_env, "DATADOG_APP_KEY");
        assert_eq!(datadog.connection["eu"].site, "datadoghq.eu");
        assert_eq!(datadog.connection["eu"].api_key_env, "DATADOG_EU_API_KEY");
        assert_eq!(datadog.connection["eu"].app_key_env, "DATADOG_APP_KEY");
    }

    #[test]
    fn datadog_connector_is_optional_and_has_no_connections_by_default() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        assert!(config.connector.datadog.connection.is_empty());
        assert_eq!(config.connector.datadog.mcp_connection, None);
    }

    #[test]
    fn parses_named_esa_connections_and_the_agent_connection_selection() {
        let toml = valid_config()
            + r#"
[connector.esa]
agent_connection = "docs"

[connector.esa.connection.docs]
team_name = "docs"

[connector.esa.connection.wiki]
team_name = "wiki"
access_token_env = "ESA_WIKI_ACCESS_TOKEN"
"#;

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let esa = config.connector.esa.expect("connector.esa present");

        assert_eq!(esa.agent_connection.as_deref(), Some("docs"));
        assert_eq!(esa.connection["docs"].team_name, "docs");
        assert_eq!(esa.connection["docs"].access_token_env, "ESA_ACCESS_TOKEN");
        assert_eq!(
            esa.connection["wiki"].access_token_env,
            "ESA_WIKI_ACCESS_TOKEN"
        );
    }

    #[test]
    fn rejects_datadog_and_esa_settings_outside_a_named_table_at_parse_time() {
        for (table, setting) in [
            ("datadog", "site = \"datadoghq.eu\""),
            ("datadog", "api_key_env = \"DATADOG_API_KEY\""),
            ("datadog", "app_key_env = \"DATADOG_APP_KEY\""),
            ("esa", "team_name = \"docs\""),
            ("esa", "access_token_env = \"ESA_ACCESS_TOKEN\""),
        ] {
            let toml = valid_config() + &format!("\n[connector.{table}]\n{setting}\n");

            let message = parse_error_message(&toml);

            assert!(
                message.contains(&format!(
                    "must sit in a named table such as `[connector.{table}.connection.default]`"
                )),
                "{setting}: {message}"
            );
        }
    }

    #[test]
    fn rejects_unknown_datadog_and_esa_connector_keys_at_parse_time() {
        for table in ["datadog", "esa"] {
            let toml = valid_config() + &format!("\n[connector.{table}]\nmcp_acount = \"x\"\n");

            let message = parse_error_message(&toml);

            assert!(
                message.contains("unknown field `mcp_acount`"),
                "{table}: {message}"
            );
        }
    }

    #[test]
    fn rejects_an_esa_connection_without_a_team_name_at_parse_time() {
        let toml = valid_config() + "\n[connector.esa.connection.docs]\n";

        let message = parse_error_message(&toml);

        assert!(message.contains("missing field `team_name`"), "{message}");
    }

    #[test]
    fn parses_work_state_github_connection_selection() {
        let toml = valid_config()
            + r#"
[work_state.github]
repository = "example-org/reili-work"
connection = "work_state"
"#;

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let github = config.work_state.github.expect("work_state.github present");

        assert_eq!(github.connection.as_deref(), Some("work_state"));
    }

    #[test]
    fn rejects_work_state_github_credentials_at_parse_time() {
        for credential in [
            "app_id = \"12345\"",
            "installation_id = \"67890\"",
            "private_key_env = \"GITHUB_APP_PRIVATE_KEY\"",
        ] {
            let toml = valid_config()
                + &format!(
                    "\n[work_state.github]\nrepository = \"example-org/reili-work\"\n{credential}\n"
                );

            let message = parse_error_message(&toml);

            assert!(message.contains("unknown field"), "{credential}: {message}");
        }
    }

    #[test]
    fn accepts_slack_authorization_with_actors_only_at_parse_time() {
        for authorization_config in [
            r#"[channel.slack.authorization]

"#,
            r#"[channel.slack.authorization.actors]
user_ids = ["U001"]

"#,
        ] {
            let toml = valid_config().replace("[ai]\n", &format!("{authorization_config}[ai]\n"));

            let config =
                parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");

            assert!(config.channel.slack.authorization.is_some());
        }
    }

    #[test]
    fn rejects_legacy_slack_authorization_channels_at_parse_time() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[channel.slack.authorization.channels]
names = ["alerts-*"]

[ai]
"#,
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("unknown field `channels`"), "{message}");
    }

    #[test]
    fn parses_slack_channels_table_with_defaults() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["incidents", "alerts-*"]
auto_response = true
auto_response_policy = "React to incidents."

[[channel.slack.channels]]
names = ["team-sre"]

[ai]
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let channels = &config.channel.slack.channels;

        assert_eq!(channels.len(), 2);
        assert_eq!(
            channels[0].names,
            vec!["incidents".to_string(), "alerts-*".to_string()]
        );
        assert!(channels[0].mention);
        assert!(channels[0].auto_response);
        assert_eq!(
            channels[0].auto_response_policy.as_deref(),
            Some("React to incidents.")
        );
        assert_eq!(channels[1].names, vec!["team-sre".to_string()]);
        assert!(channels[1].mention);
        assert!(!channels[1].auto_response);
        assert_eq!(channels[1].auto_response_policy, None);
    }

    #[test]
    fn defaults_slack_channels_to_empty_when_omitted() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        assert!(config.channel.slack.channels.is_empty());
    }

    #[test]
    fn rejects_empty_slack_channel_auto_response_policy_at_parse_time() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[[channel.slack.channels]]
names = ["alerts-*"]
auto_response = true
auto_response_policy = "   "

[ai]
"#,
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
    }

    #[test]
    fn defaults_memory_to_none_when_omitted() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        assert!(config.memory.slack.is_none());
    }

    #[test]
    fn parses_memory_slack_canvas_block() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[memory.slack]
canvas_id = "F0CANVAS"
cap = 5

[ai]
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let slack = config.memory.slack.expect("memory.slack present");

        assert_eq!(slack.canvas_id, "F0CANVAS");
        assert_eq!(slack.cap, Some(5));
    }

    #[test]
    fn rejects_empty_memory_canvas_id_at_parse_time() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[memory.slack]
canvas_id = "  "

[ai]
"#,
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
    }

    #[test]
    fn defaults_tracing_otlp_to_none_when_omitted() {
        let config =
            parse_file_config(Path::new(TEST_PATH), &valid_config()).expect("parse should succeed");

        assert!(config.tracing.otlp.is_none());
    }

    #[test]
    fn parses_tracing_otlp_block() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[tracing.otlp]
endpoint = "http://localhost:4317"
service_name = "reili-dev"

[ai]
"#,
        );

        let config = parse_file_config(Path::new(TEST_PATH), &toml).expect("parse should succeed");
        let otlp = config.tracing.otlp.expect("tracing.otlp present");

        assert_eq!(otlp.endpoint, "http://localhost:4317");
        assert_eq!(otlp.service_name, Some("reili-dev".to_string()));
    }

    #[test]
    fn rejects_empty_tracing_otlp_endpoint_at_parse_time() {
        let toml = valid_config().replace(
            "[ai]\n",
            r#"[tracing.otlp]
endpoint = "  "

[ai]
"#,
        );

        let message = parse_error_message(&toml);

        assert!(message.contains("non-empty string"), "{message}");
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

[connector.github.connection.default]
app_id = "12345"
installation_id = "67890"
"#
        .to_string()
    }
}
