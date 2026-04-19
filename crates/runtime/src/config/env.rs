#[cfg_attr(test, mockall::automock)]
pub(crate) trait EnvironmentReader {
    fn get(&self, name: &str) -> Option<String>;
}

pub(crate) struct ProcessEnvironment;

impl EnvironmentReader for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

use super::ConfigError;
use crate::config::SecretString;

pub(crate) fn read_required_secret(
    env: &dyn EnvironmentReader,
    env_ref: &str,
    field: &str,
) -> Result<SecretString, ConfigError> {
    let env_name = read_trimmed_env_reference(env_ref, field)?;
    read_optional_non_empty_env(env, &env_name)
        .map(SecretString::new)
        .ok_or_else(|| ConfigError::MissingRequiredEnv {
            env: env_name,
            field: field.to_string(),
        })
}

pub(crate) fn read_trimmed_env_reference(
    env_ref: &str,
    field: &str,
) -> Result<String, ConfigError> {
    let trimmed = env_ref.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidValue {
            field: field.to_string(),
            message: "must reference a non-empty environment variable name".to_string(),
        });
    }

    Ok(trimmed.to_string())
}

fn read_optional_non_empty_env(env: &dyn EnvironmentReader, name: &str) -> Option<String> {
    match env.get(name) {
        Some(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{EnvironmentReader, read_required_secret, read_trimmed_env_reference};
    use crate::config::ConfigError;

    struct FixedEnvironment {
        values: HashMap<String, String>,
    }

    impl EnvironmentReader for FixedEnvironment {
        fn get(&self, name: &str) -> Option<String> {
            self.values.get(name).cloned()
        }
    }

    #[test]
    fn resolves_required_secret_from_env_ref() {
        let env = FixedEnvironment {
            values: HashMap::from([("TOKEN".to_string(), "secret-value".to_string())]),
        };

        let secret = read_required_secret(&env, "TOKEN", "channel.slack.auth.bot_token_env")
            .expect("secret");

        assert_eq!(secret.expose(), "secret-value");
    }

    #[test]
    fn reports_missing_required_env_with_field_context() {
        let env = FixedEnvironment {
            values: HashMap::new(),
        };

        let error = read_required_secret(&env, "TOKEN", "channel.slack.auth.bot_token_env")
            .expect_err("missing env should fail");

        match error {
            ConfigError::MissingRequiredEnv { env, field } => {
                assert_eq!(env, "TOKEN");
                assert_eq!(field, "channel.slack.auth.bot_token_env");
            }
            other => panic!("expected missing-env error, got {other}"),
        }
    }

    #[test]
    fn rejects_blank_env_reference() {
        let error = read_trimmed_env_reference("   ", "channel.slack.auth.bot_token_env")
            .expect_err("blank env ref should fail");

        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "channel.slack.auth.bot_token_env");
                assert!(message.contains("non-empty"));
            }
            other => panic!("expected invalid-value error, got {other}"),
        }
    }
}
