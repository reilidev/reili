use std::collections::BTreeMap;

use super::LogFieldValue;

pub type LogFields = BTreeMap<String, LogFieldValue>;

pub fn log_fields<K, V, I>(entries: I) -> LogFields
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<LogFieldValue>,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
}
