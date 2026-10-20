use std::collections::BTreeMap;

use super::ConfigError;

/// Which of a connector's named instances (`[connector.github.connection.<name>]`,
/// `[connector.jira.connection.<name>]`, ...) a setting refers to. `selected` is the name written in
/// the referring setting at `selector_field`; it may be omitted only while a single instance is
/// defined.
pub(super) struct NamedSelection<'a, T> {
    pub instances: &'a BTreeMap<String, T>,
    pub selected: Option<&'a str>,
    /// Config path of the setting that holds `selected`, reported in errors.
    pub selector_field: &'a str,
    /// What an instance is called in messages, e.g. `GitHub connection`.
    pub kind: &'a str,
    /// Where instances are defined, e.g. `[connector.github.connection.<name>]`.
    pub table: &'a str,
}

pub(super) fn select_named<'a, T>(
    selection: &NamedSelection<'a, T>,
) -> Result<(&'a str, &'a T), ConfigError> {
    let NamedSelection {
        instances,
        selected,
        selector_field,
        kind,
        table,
    } = selection;
    let defined_names = || {
        instances
            .keys()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let invalid = |message: String| ConfigError::InvalidValue {
        field: (*selector_field).to_string(),
        message,
    };

    if let Some(name) = selected {
        return instances
            .get_key_value(*name)
            .map(|(name, instance)| (name.as_str(), instance))
            .ok_or_else(|| {
                invalid(format!(
                    "unknown {kind} `{name}`; defined in {table}: {}",
                    defined_names()
                ))
            });
    }

    let mut candidates = instances.iter();
    match (candidates.next(), candidates.next()) {
        (Some((name, instance)), None) => Ok((name.as_str(), instance)),
        (Some(_), Some(_)) => Err(invalid(format!(
            "required when several {kind}s are defined; choose one of: {}",
            defined_names()
        ))),
        (None, _) => Err(invalid(format!("no {kind} is defined in {table}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{NamedSelection, select_named};
    use crate::config::ConfigError;

    fn instances(names: &[&str]) -> BTreeMap<String, u32> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| ((*name).to_string(), index as u32))
            .collect()
    }

    fn select<'a>(
        instances: &'a BTreeMap<String, u32>,
        selected: Option<&'a str>,
    ) -> Result<(&'a str, &'a u32), ConfigError> {
        select_named(&NamedSelection {
            instances,
            selected,
            selector_field: "work_state.jira.connection",
            kind: "Jira connection",
            table: "[connector.jira.connection.<name>]",
        })
    }

    fn invalid_message(error: ConfigError) -> String {
        match error {
            ConfigError::InvalidValue { field, message } => {
                assert_eq!(field, "work_state.jira.connection");
                message
            }
            other => panic!("expected invalid value, got {other}"),
        }
    }

    #[test]
    fn selects_the_only_instance_when_none_is_named() {
        let instances = instances(&["default"]);

        assert_eq!(select(&instances, None).expect("select").0, "default");
    }

    #[test]
    fn selects_the_named_instance_among_several() {
        let instances = instances(&["reader", "work_state"]);

        let (name, value) = select(&instances, Some("work_state")).expect("select");

        assert_eq!((name, *value), ("work_state", 1));
    }

    #[test]
    fn requires_a_name_when_several_instances_are_defined() {
        let instances = instances(&["reader", "work_state"]);

        let message = invalid_message(select(&instances, None).expect_err("ambiguous"));

        assert_eq!(
            message,
            "required when several Jira connections are defined; choose one of: `reader`, `work_state`"
        );
    }

    #[test]
    fn rejects_an_unknown_name_and_lists_the_defined_ones() {
        let instances = instances(&["reader", "work_state"]);

        let message = invalid_message(select(&instances, Some("writer")).expect_err("unknown"));

        assert_eq!(
            message,
            "unknown Jira connection `writer`; defined in [connector.jira.connection.<name>]: `reader`, `work_state`"
        );
    }

    #[test]
    fn rejects_a_selection_when_nothing_is_defined() {
        let instances = instances(&[]);

        let message = invalid_message(select(&instances, None).expect_err("none defined"));

        assert_eq!(
            message,
            "no Jira connection is defined in [connector.jira.connection.<name>]"
        );
    }
}
