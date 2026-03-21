use super::{LogFields, LogLevel};

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub level: LogLevel,
    pub event: &'static str,
    pub fields: LogFields,
}
