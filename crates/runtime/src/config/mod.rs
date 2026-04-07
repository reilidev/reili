mod env;
mod file;
mod loader;
mod model;

use std::path::PathBuf;

use thiserror::Error;

pub use loader::{ConfigLoadOptions, load_app_config};
pub use model::{
    AppConfig, BedrockLlmConfig, GitHubConfig, LlmConfig, LlmProviderConfig, OpenAiLlmConfig,
    SecretString, SlackConnectionMode, VertexAiLlmConfig,
};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to determine current working directory: {source}")]
    CurrentDir { source: std::io::Error },
    #[error("Failed to read config file `{path}`: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to parse TOML config `{path}`: {message}")]
    ParseToml { path: PathBuf, message: String },
    #[error("Unsupported config version `{found}` at `version`")]
    UnsupportedVersion { found: u32 },
    #[error("Missing required environment variable `{env}` referenced by `{field}`")]
    MissingRequiredEnv { env: String, field: String },
    #[error("Invalid value for `{field}`: {message}")]
    InvalidValue { field: String, message: String },
}
