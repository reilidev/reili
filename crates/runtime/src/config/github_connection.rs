use std::collections::BTreeMap;

use super::ConfigError;
use super::env::{EnvironmentReader, read_required_secret};
use super::file::GitHubConnectionFileConfig;
use super::named_connector::{NamedSelection, select_named};
use crate::config::SecretString;

/// Which of the named `[connector.github.connection.<name>]` connections a setting refers to. `selected` is the
/// name written in the referring setting at `selector_field`; it may be omitted only while a
/// single connection is defined.
pub(super) struct GitHubConnectionReference<'a> {
    pub connections: &'a BTreeMap<String, GitHubConnectionFileConfig>,
    pub selected: Option<&'a str>,
    pub selector_field: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitHubAppCredentials {
    pub app_id: String,
    pub installation_id: u32,
    pub private_key: SecretString,
}

/// Resolves the referenced connection and reads its private key. Only the referenced connection's
/// environment variable is read, so a connection that nothing selects never blocks startup.
pub(super) fn resolve_github_connection(
    reference: &GitHubConnectionReference<'_>,
    env: &dyn EnvironmentReader,
) -> Result<GitHubAppCredentials, ConfigError> {
    let (name, connection) = select_named(&NamedSelection {
        instances: reference.connections,
        selected: reference.selected,
        selector_field: reference.selector_field,
        kind: "GitHub connection",
        table: "[connector.github.connection.<name>]",
    })?;
    let private_key = read_required_secret(
        env,
        &connection.private_key_env,
        &format!("connector.github.connection.{name}.private_key_env"),
    )?;
    Ok(GitHubAppCredentials {
        app_id: connection.app_id.clone(),
        installation_id: connection.installation_id,
        private_key: normalize_multiline_secret(private_key),
    })
}

fn normalize_multiline_secret(secret: SecretString) -> SecretString {
    SecretString::new(secret.expose().replace("\\n", "\n"))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::{GitHubConnectionReference, resolve_github_connection};
    use crate::config::ConfigError;
    use crate::config::env::EnvironmentReader;
    use crate::config::file::GitHubConnectionFileConfig;

    struct FixedEnvironment(HashMap<&'static str, &'static str>);

    impl EnvironmentReader for FixedEnvironment {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).map(ToString::to_string)
        }
    }

    fn connection(
        app_id: &str,
        installation_id: u32,
        private_key_env: &str,
    ) -> GitHubConnectionFileConfig {
        GitHubConnectionFileConfig {
            app_id: app_id.to_string(),
            installation_id,
            private_key_env: private_key_env.to_string(),
        }
    }

    fn connections(
        entries: Vec<(&str, GitHubConnectionFileConfig)>,
    ) -> BTreeMap<String, GitHubConnectionFileConfig> {
        entries
            .into_iter()
            .map(|(name, connection)| (name.to_string(), connection))
            .collect()
    }

    fn environment() -> FixedEnvironment {
        FixedEnvironment(HashMap::from([
            ("READER_KEY", "reader\\nkey"),
            ("WORK_STATE_KEY", "work-state-key"),
        ]))
    }

    fn two_connections() -> BTreeMap<String, GitHubConnectionFileConfig> {
        connections(vec![
            ("reader", connection("100", 1, "READER_KEY")),
            ("work_state", connection("200", 2, "WORK_STATE_KEY")),
        ])
    }

    fn reference<'a>(
        connections: &'a BTreeMap<String, GitHubConnectionFileConfig>,
        selected: Option<&'a str>,
    ) -> GitHubConnectionReference<'a> {
        GitHubConnectionReference {
            connections,
            selected,
            selector_field: "work_state.github.connection",
        }
    }

    #[test]
    fn selects_the_only_defined_connection_when_none_is_named() {
        let connections = connections(vec![("default", connection("100", 1, "READER_KEY"))]);

        let credentials = resolve_github_connection(&reference(&connections, None), &environment())
            .expect("resolve");

        assert_eq!(credentials.app_id, "100");
        assert_eq!(credentials.installation_id, 1);
    }

    #[test]
    fn selects_the_named_connection_and_reads_only_its_private_key() {
        let connections = two_connections();
        let env = FixedEnvironment(HashMap::from([("WORK_STATE_KEY", "work-state-key")]));

        let credentials =
            resolve_github_connection(&reference(&connections, Some("work_state")), &env)
                .expect("resolve");

        assert_eq!(credentials.app_id, "200");
        assert_eq!(credentials.installation_id, 2);
        assert_eq!(credentials.private_key.expose(), "work-state-key");
    }

    #[test]
    fn expands_escaped_newlines_in_the_private_key() {
        let connections = two_connections();

        let credentials =
            resolve_github_connection(&reference(&connections, Some("reader")), &environment())
                .expect("resolve");

        assert_eq!(credentials.private_key.expose(), "reader\nkey");
    }

    #[test]
    fn reports_the_selected_apps_missing_private_key_env() {
        let connections = two_connections();
        let env = FixedEnvironment(HashMap::new());

        let error = resolve_github_connection(&reference(&connections, Some("work_state")), &env)
            .expect_err("missing private key must fail");

        match error {
            ConfigError::MissingRequiredEnv { env, field } => {
                assert_eq!(env, "WORK_STATE_KEY");
                assert_eq!(
                    field,
                    "connector.github.connection.work_state.private_key_env"
                );
            }
            other => panic!("expected missing env, got {other}"),
        }
    }
}
