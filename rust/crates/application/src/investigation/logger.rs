use std::collections::BTreeMap;

pub trait InvestigationLogger: Send + Sync {
    fn info(&self, message: &str, meta: BTreeMap<String, String>);
    fn warn(&self, message: &str, meta: BTreeMap<String, String>);
    fn error(&self, message: &str, meta: BTreeMap<String, String>);
}
