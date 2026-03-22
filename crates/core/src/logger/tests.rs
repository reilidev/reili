use super::{LogEntry, LogFieldValue, LogLevel, Logger, log_fields, port::MockLogger};

struct LoggerHarness {
    inner: MockLogger,
}

impl Logger for LoggerHarness {
    fn log(&self, entry: LogEntry) {
        self.inner.log(entry);
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
    let expected = LogEntry {
        level: LogLevel::Info,
        event: "slack_stream_started",
        fields: log_fields([("channel", "C123")]),
    };
    let mut inner = MockLogger::new();

    inner
        .expect_log()
        .withf(move |entry| entry == &expected)
        .times(1)
        .return_const(());

    let logger = LoggerHarness { inner };
    logger.info("slack_stream_started", log_fields([("channel", "C123")]));
}
