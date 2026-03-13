use std::fmt as std_fmt;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::fmt::{self as tracing_fmt, FmtContext};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Registry};

pub fn init_json_logger() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    let subscriber = build_json_subscriber(std::io::stderr);
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

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;
    use tracing_subscriber::fmt::writer::MakeWriter;

    use super::build_json_subscriber;

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
            tracing::info!(job_id = "job-1", "investigation started");
            tracing::warn!(retry_count = 2, "investigation retried");
            tracing::error!(job_id = "job-1", "investigation failed");
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let logs: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("parse json log line"))
            .collect();

        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0]["level"], "INFO");
        assert_eq!(logs[0]["message"], "investigation started");
        assert_eq!(logs[0]["job_id"], "job-1");
        assert_eq!(logs[1]["level"], "WARN");
        assert_eq!(logs[1]["message"], "investigation retried");
        assert_eq!(logs[1]["retry_count"], 2);
        assert_eq!(logs[2]["level"], "ERROR");
        assert_eq!(logs[2]["message"], "investigation failed");
        assert_eq!(logs[2]["job_id"], "job-1");
    }

    #[test]
    fn writes_meta_as_json_object() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                message = "investigation started",
                meta = tracing::field::display(serde_json::json!({
                    "jobId": "job-1",
                    "attempt": "1"
                })),
            );
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let log: Value = serde_json::from_str(output.trim()).expect("parse json log line");

        assert_eq!(log["message"], "investigation started");
        assert_eq!(log["meta"]["jobId"], "job-1");
        assert_eq!(log["meta"]["attempt"], "1");
        assert!(log["meta"].is_object());
    }
}
