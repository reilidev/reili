use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

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
    pub port: u32,
}

impl Default for ServerFileConfig {
    fn default() -> Self {
        Self { port: 3000 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct ConversationFileConfig {
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
            language: "English".to_string(),
            additional_system_prompt: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ChannelFileConfig {
    pub slack: SlackFileConfig,
}

fn default_true() -> bool {
    true
}

fn default_github_mcp_url() -> String {
    "https://api.githubcopilot.com/mcp/".to_string()
}

fn default_github_app_private_key_env() -> String {
    "GITHUB_APP_PRIVATE_KEY".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackFileConfig {
    #[serde(default = "default_true")]
    pub socket_mode: bool,
    #[serde(default)]
    pub auth: SlackAuthFileConfig,
    pub socket: Option<SlackSocketFileConfig>,
    pub http: Option<SlackHttpFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct SlackAuthFileConfig {
    pub bot_token_env: String,
}

impl Default for SlackAuthFileConfig {
    fn default() -> Self {
        Self {
            bot_token_env: "SLACK_BOT_TOKEN".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackSocketFileConfig {
    pub app_token_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackHttpFileConfig {
    pub signing_secret_env: Option<String>,
}

impl Default for SlackSocketFileConfig {
    fn default() -> Self {
        Self {
            app_token_env: Some("SLACK_APP_TOKEN".to_string()),
        }
    }
}

impl Default for SlackHttpFileConfig {
    fn default() -> Self {
        Self {
            signing_secret_env: Some("SLACK_SIGNING_SECRET".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AiFileConfig {
    pub default_backend: String,
    pub backends: BTreeMap<String, AiBackendFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AiBackendFileConfig {
    pub provider: Option<String>,
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    pub model_id: Option<String>,
    pub aws_profile: Option<String>,
    pub aws_region: Option<String>,
    pub project_id: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ConnectorFileConfig {
    #[serde(default)]
    pub datadog: DatadogConnectorFileConfig,
    pub github: GitHubConnectorFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct DatadogConnectorFileConfig {
    pub site: String,
    pub api_key_env: String,
    pub app_key_env: String,
}

impl Default for DatadogConnectorFileConfig {
    fn default() -> Self {
        Self {
            site: "datadoghq.com".to_string(),
            api_key_env: "DATADOG_API_KEY".to_string(),
            app_key_env: "DATADOG_APP_KEY".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubConnectorFileConfig {
    #[serde(default = "default_github_mcp_url")]
    pub mcp_url: String,
    pub search_scope_org: String,
    pub app: GitHubAppFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubAppFileConfig {
    pub app_id: String,
    pub installation_id: String,
    #[serde(default = "default_github_app_private_key_env")]
    pub private_key_env: String,
}

pub(crate) fn parse_file_config(path: &Path, contents: &str) -> Result<FileConfig, ConfigError> {
    toml::from_str(contents).map_err(|error| ConfigError::ParseToml {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}
