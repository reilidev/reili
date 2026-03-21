use std::sync::Mutex;

use super::{LogEntry, LogFieldValue, LogLevel, Logger, log_fields};

#[derive(Default)]
struct RecordingLogger {
    entries: Mutex<Vec<LogEntry>>,
}

impl RecordingLogger {
    fn entries(&self) -> Vec<LogEntry> {
        self.entries.lock().expect("lock log entries").clone()
    }
}

impl Logger for RecordingLogger {
    fn log(&self, entry: LogEntry) {
        self.entries.lock().expect("lock log entries").push(entry);
    }
}

#[test]
fn log_fields_preserves_typed_values() {
    let fields = log_fields([
        ("service", LogFieldValue::from("slack")),
        ("attempt", LogFieldValue::from(2_u64)),
        ("success", LogFieldValue::from(true)),
    ]);

    assert_eq!(
        fields.get("service").and_then(LogFieldValue::as_str),
        Some("slack")
    );
    assert_eq!(
        fields.get("attempt").and_then(LogFieldValue::as_u64),
        Some(2)
    );
    assert_eq!(
        fields.get("success").and_then(LogFieldValue::as_bool),
        Some(true)
    );
}

#[test]
fn info_helper_creates_log_entry() {
    let logger = RecordingLogger::default();

    logger.info("slack_stream_started", log_fields([("channel", "C123")]));

    assert_eq!(
        logger.entries(),
        vec![LogEntry {
            level: LogLevel::Info,
            event: "slack_stream_started",
            fields: log_fields([("channel", "C123")]),
        }]
    );
}
