use std::fmt as std_fmt;

use chrono::{SecondsFormat, Utc};
use reili_core::logger::{LogEntry, LogFieldValue, LogFields, LogLevel, Logger};
use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::fmt::{self as tracing_fmt, FmtContext};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Registry};

#[derive(Debug, Default)]
pub struct TracingLogger;

impl Logger for TracingLogger {
    fn log(&self, entry: LogEntry) {
        let meta_json = Value::Object(log_fields_to_json(entry.fields));

        match entry.level {
            LogLevel::Debug => {
                tracing::debug!(
                    message = entry.event,
                    meta = tracing::field::display(meta_json)
                );
            }
            LogLevel::Info => {
                tracing::info!(
                    message = entry.event,
                    meta = tracing::field::display(meta_json)
                );
            }
            LogLevel::Warn => {
                tracing::warn!(
                    message = entry.event,
                    meta = tracing::field::display(meta_json)
                );
            }
            LogLevel::Error => {
                tracing::error!(
                    message = entry.event,
                    meta = tracing::field::display(meta_json)
                );
            }
        }
    }
}

pub fn init_json_logger() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    let subscriber = build_json_subscriber(std::io::stdout);
    tracing::subscriber::set_global_default(subscriber)
}

pub fn build_json_subscriber<W>(make_writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    Registry::default().with(default_env_filter()).with(
        tracing_fmt::layer()
            .event_format(JsonEventFormatter)
            .with_ansi(false)
            .with_writer(make_writer),
    )
}

const META_FIELD_NAME: &str = "meta";

struct JsonEventFormatter;

impl<S, N> FormatEvent<S, N> for JsonEventFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std_fmt::Result {
        let mut fields = Map::from_iter([
            (
                "timestamp".to_string(),
                Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
            ),
            (
                "level".to_string(),
                Value::String(event.metadata().level().as_str().to_string()),
            ),
        ]);
        let mut visitor = JsonEventFieldVisitor::default();
        event.record(&mut visitor);
        fields.extend(visitor.fields);

        let json = serde_json::to_string(&fields).map_err(|_| std_fmt::Error)?;
        std_fmt::Write::write_str(&mut writer, &json)?;
        std_fmt::Write::write_char(&mut writer, '\n')
    }
}

#[derive(Default)]
struct JsonEventFieldVisitor {
    fields: Map<String, Value>,
}

impl JsonEventFieldVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        self.fields.insert(
            field.name().to_string(),
            normalize_field_value(field, value),
        );
    }
}

impl Visit for JsonEventFieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(
            field,
            Number::from_f64(value).map_or(Value::Null, Value::Number),
        );
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::Number(Number::from(value)));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::Number(Number::from(value)));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::String(value.to_string()));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field, Value::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std_fmt::Debug) {
        self.insert(field, Value::String(format!("{value:?}")));
    }
}

fn normalize_field_value(field: &Field, value: Value) -> Value {
    if field.name() != META_FIELD_NAME {
        return value;
    }

    let Value::String(string_value) = value else {
        return value;
    };

    match serde_json::from_str::<Value>(&string_value) {
        Ok(parsed_meta) => parsed_meta,
        Err(_) => Value::String(string_value),
    }
}

fn default_env_filter() -> EnvFilter {
    match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new("info"),
    }
}

fn log_fields_to_json(fields: LogFields) -> Map<String, Value> {
    fields
        .into_iter()
        .map(|(key, value)| (key, log_field_value_to_json(value)))
        .collect()
}

fn log_field_value_to_json(value: LogFieldValue) -> Value {
    match value {
        LogFieldValue::String(value) => Value::String(value),
        LogFieldValue::I64(value) => Value::Number(Number::from(value)),
        LogFieldValue::U64(value) => Value::Number(Number::from(value)),
        LogFieldValue::Bool(value) => Value::Bool(value),
        LogFieldValue::F64(value) => Number::from_f64(value).map_or(Value::Null, Value::Number),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use reili_core::logger::{LogFieldValue, Logger, log_fields};
    use serde_json::Value;
    use tracing_subscriber::fmt::writer::MakeWriter;

    use super::{TracingLogger, build_json_subscriber};

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn snapshot(&self) -> Vec<u8> {
            self.0.lock().expect("lock shared buffer").clone()
        }
    }

    struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBufferWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut lock = self.0.lock().expect("lock shared buffer");
            lock.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for SharedBuffer {
        type Writer = SharedBufferWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            SharedBufferWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn writes_json_logs_for_info_warn_and_error() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(job_id = "job-1", "task started");
            tracing::warn!(retry_count = 2, "task retried");
            tracing::error!(job_id = "job-1", "task failed");
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let logs: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("parse json log line"))
            .collect();

        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0]["level"], "INFO");
        assert_eq!(logs[0]["message"], "task started");
        assert_eq!(logs[0]["job_id"], "job-1");
        assert_eq!(logs[1]["level"], "WARN");
        assert_eq!(logs[1]["message"], "task retried");
        assert_eq!(logs[1]["retry_count"], 2);
        assert_eq!(logs[2]["level"], "ERROR");
        assert_eq!(logs[2]["message"], "task failed");
        assert_eq!(logs[2]["job_id"], "job-1");
    }

    #[test]
    fn writes_meta_as_json_object() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                message = "task started",
                meta = tracing::field::display(serde_json::json!({
                    "jobId": "job-1",
                    "attempt": "1"
                })),
            );
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let log: Value = serde_json::from_str(output.trim()).expect("parse json log line");

        assert_eq!(log["message"], "task started");
        assert_eq!(log["meta"]["jobId"], "job-1");
        assert_eq!(log["meta"]["attempt"], "1");
        assert!(log["meta"].is_object());
    }

    #[test]
    fn tracing_logger_serializes_typed_log_fields() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());
        let logger = TracingLogger;

        tracing::subscriber::with_default(subscriber, || {
            logger.info(
                "slack_stream_started",
                log_fields([
                    ("channel", LogFieldValue::from("C123")),
                    ("attempt", LogFieldValue::from(2_u64)),
                    ("success", LogFieldValue::from(true)),
                ]),
            );
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let log: Value = serde_json::from_str(output.trim()).expect("parse json log line");

        assert_eq!(log["message"], "slack_stream_started");
        assert_eq!(log["meta"]["channel"], "C123");
        assert_eq!(log["meta"]["attempt"], 2);
        assert_eq!(log["meta"]["success"], true);
    }
}
