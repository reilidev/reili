mod env;
mod error;
mod file;
mod loader;
mod model;

pub use reili_core::messaging::slack::SlackChannelNamePattern;
pub use reili_core::secret::SecretString;

pub use error::ConfigError;
pub use loader::{ConfigLoadOptions, load_app_config};
pub use model::{
    AppConfig, BedrockAwsConfig, BedrockLlmConfig, BedrockMantleAuthConfig, BedrockMantleLlmConfig,
    EsaConfig, GitHubConfig, JiraConfig, JudgeProviderConfig, LlmConfig, LlmProviderConfig,
    OpenAiLlmConfig, OtlpTracingConfig, SlackAuthorizationActors, SlackAuthorizationConfig,
    SlackCanvasMemoryConfig, SlackChannelConfig, SlackConnectionMode, VertexAiLlmConfig,
    WebSearchProviderConfig,
};
