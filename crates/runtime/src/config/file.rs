use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::ConfigError;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct FileConfig {
    pub version: u32,
    pub server: ServerFileConfig,
    pub conversation: ConversationFileConfig,
    pub channel: ChannelFileConfig,
    pub ai: AiFileConfig,
    pub connector: ConnectorFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ServerFileConfig {
    pub port: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ConversationFileConfig {
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ChannelFileConfig {
    pub slack: SlackFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackFileConfig {
    pub mode: Option<String>,
    pub socket_mode: Option<bool>,
    pub auth: SlackAuthFileConfig,
    pub socket: Option<SlackSocketFileConfig>,
    pub http: Option<SlackHttpFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackAuthFileConfig {
    pub bot_token_env: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackSocketFileConfig {
    pub app_token_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct SlackHttpFileConfig {
    pub signing_secret_env: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AiFileConfig {
    pub default_backend: String,
    pub backends: BTreeMap<String, AiBackendFileConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AiBackendFileConfig {
    pub provider: Option<String>,
    pub task_runner_model: Option<String>,
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
    pub datadog: DatadogConnectorFileConfig,
    pub github: GitHubConnectorFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DatadogConnectorFileConfig {
    pub site: String,
    pub api_key_env: String,
    pub app_key_env: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubConnectorFileConfig {
    pub mcp_url: String,
    pub search_scope_org: String,
    pub app: GitHubAppFileConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubAppFileConfig {
    pub app_id: String,
    pub installation_id: String,
    pub private_key_env: String,
}

pub(crate) fn parse_file_config(path: &Path, contents: &str) -> Result<FileConfig, ConfigError> {
    toml::from_str(contents).map_err(|error| ConfigError::ParseToml {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}
