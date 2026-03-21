use super::{LogEntry, LogFields, LogLevel};

pub trait Logger: Send + Sync {
    fn log(&self, entry: LogEntry);

    fn debug(&self, event: &'static str, fields: LogFields) {
        self.log(LogEntry {
            level: LogLevel::Debug,
            event,
            fields,
        });
    }

    fn info(&self, event: &'static str, fields: LogFields) {
        self.log(LogEntry {
            level: LogLevel::Info,
            event,
            fields,
        });
    }

    fn warn(&self, event: &'static str, fields: LogFields) {
        self.log(LogEntry {
            level: LogLevel::Warn,
            event,
            fields,
        });
    }

    fn error(&self, event: &'static str, fields: LogFields) {
        self.log(LogEntry {
            level: LogLevel::Error,
            event,
            fields,
        });
    }
}
