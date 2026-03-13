use serde_json::{Map, Value};

pub type InvestigationLogMeta = Map<String, Value>;

pub trait InvestigationLogger: Send + Sync {
    fn info(&self, message: &str, meta: InvestigationLogMeta);
    fn warn(&self, message: &str, meta: InvestigationLogMeta);
    fn error(&self, message: &str, meta: InvestigationLogMeta);
}

pub fn string_log_meta<K, V, I>(entries: I) -> InvestigationLogMeta
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.into(), Value::String(value.into())))
        .collect()
}
