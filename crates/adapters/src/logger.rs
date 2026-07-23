use std::fmt as std_fmt;

use chrono::{SecondsFormat, Utc};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{ExporterBuildError, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use reili_core::logger::{LogEntry, LogFieldValue, LogFields, LogLevel, Logger};
use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::fmt::{self as tracing_fmt, FmtContext};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Registry};

/// Instrumentation scope name attached to spans exported to the OTLP collector.
const OTEL_TRACER_SCOPE: &str = "reili";
/// `rig`'s own GenAI-semantic-convention spans (`rig::agent_chat`, `rig::completions`, ...).
const OTEL_SPAN_TARGET: &str = "rig";
/// [`TracingLogger::log`]'s target — `tracing` tags every event with its call site's module
/// regardless of caller, so all `TracingLogger` output shares this one target.
const OTEL_APP_LOG_TARGET: &str = "reili_adapters::logger";

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

#[derive(Debug, Clone)]
pub struct OtlpTracingConfig {
    pub endpoint: String,
    pub service_name: String,
}

#[derive(Debug)]
pub enum TracingInitError {
    ExporterBuild(ExporterBuildError),
    SetGlobalDefault(tracing::subscriber::SetGlobalDefaultError),
}

impl std_fmt::Display for TracingInitError {
    fn fmt(&self, f: &mut std_fmt::Formatter<'_>) -> std_fmt::Result {
        match self {
            Self::ExporterBuild(error) => write!(f, "Failed to build OTLP span exporter: {error}"),
            Self::SetGlobalDefault(error) => {
                write!(f, "Failed to install global tracing subscriber: {error}")
            }
        }
    }
}

impl std::error::Error for TracingInitError {}

impl From<ExporterBuildError> for TracingInitError {
    fn from(error: ExporterBuildError) -> Self {
        Self::ExporterBuild(error)
    }
}

impl From<tracing::subscriber::SetGlobalDefaultError> for TracingInitError {
    fn from(error: tracing::subscriber::SetGlobalDefaultError) -> Self {
        Self::SetGlobalDefault(error)
    }
}

/// Flushes buffered spans to the OTLP collector on shutdown. A no-op when OTLP export was not
/// configured, so callers can unconditionally hold and call [`TracingShutdown::shutdown`].
#[must_use]
pub struct TracingShutdown {
    tracer_provider: Option<SdkTracerProvider>,
}

impl TracingShutdown {
    pub fn shutdown(self) {
        let Some(tracer_provider) = self.tracer_provider else {
            return;
        };

        if let Err(error) = tracer_provider.shutdown() {
            tracing::warn!(
                error = %error,
                "Failed to shut down OpenTelemetry tracer provider"
            );
        }
    }
}

pub fn init_json_logger(
    otlp: Option<OtlpTracingConfig>,
) -> Result<TracingShutdown, TracingInitError> {
    let (tracer, tracer_provider) = match &otlp {
        Some(config) => {
            let (provider, tracer) = build_otel_tracer(config)?;
            (Some(tracer), Some(provider))
        }
        None => (None, None),
    };

    let otel_layer = tracer.map(|tracer| {
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(otel_span_filter())
    });

    let subscriber = build_json_subscriber(std::io::stdout).with(otel_layer);
    tracing::subscriber::set_global_default(subscriber)?;

    Ok(TracingShutdown { tracer_provider })
}

/// Targets exported to the OTLP collector, each at `INFO`, independent of `RUST_LOG` — quieting
/// stdout must never silently drop trace data. `tracing-opentelemetry` only attaches an event to a
/// currently active span, so `TracingLogger` events export only while a `rig` span is open, which
/// is what keeps this from becoming a firehose of every stdout log line. Reili's own target keeps
/// the `reili_` prefix so it still passes stdout's default `reili_=info` directive (see
/// `default_env_filter`).
fn otel_span_filter() -> Targets {
    Targets::new()
        .with_target(OTEL_SPAN_TARGET, tracing::Level::INFO)
        .with_target(OTEL_APP_LOG_TARGET, tracing::Level::INFO)
}

fn build_otel_tracer(
    config: &OtlpTracingConfig,
) -> Result<(SdkTracerProvider, SdkTracer), ExporterBuildError> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(config.endpoint.clone())
        .build()?;

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    let tracer = tracer_provider.tracer(OTEL_TRACER_SCOPE);

    Ok((tracer_provider, tracer))
}

pub fn build_json_subscriber<W>(
    make_writer: W,
) -> impl tracing::Subscriber + for<'span> LookupSpan<'span> + Send + Sync
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    Registry::default().with(
        tracing_fmt::layer()
            .event_format(JsonEventFormatter)
            .with_ansi(false)
            .with_writer(make_writer)
            .with_filter(default_env_filter()),
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
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,reili_=info"))
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
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};

    use super::{
        OTEL_APP_LOG_TARGET, OTEL_SPAN_TARGET, TracingLogger, build_json_subscriber,
        otel_span_filter,
    };

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
    fn writes_json_logs_for_warn_and_error() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(retry_count = 2, "task retried");
            tracing::error!(job_id = "job-1", "task failed");
        });

        let output = String::from_utf8(buffer.snapshot()).expect("decode buffered logs");
        let logs: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("parse json log line"))
            .collect();

        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0]["level"], "WARN");
        assert_eq!(logs[0]["message"], "task retried");
        assert_eq!(logs[0]["retry_count"], 2);
        assert_eq!(logs[1]["level"], "ERROR");
        assert_eq!(logs[1]["message"], "task failed");
        assert_eq!(logs[1]["job_id"], "job-1");
    }

    #[test]
    fn writes_meta_as_json_object() {
        let buffer = SharedBuffer::default();
        let subscriber = build_json_subscriber(buffer.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
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
            logger.warn(
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

    #[derive(Clone, Default)]
    struct TargetCapture(Arc<Mutex<Vec<&'static str>>>);

    impl<S> Layer<S> for TargetCapture
    where
        S: tracing::Subscriber,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            self.0
                .lock()
                .expect("lock captured targets")
                .push(event.metadata().target());
        }
    }

    #[test]
    fn tracing_logger_events_share_the_otel_app_log_target() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(TargetCapture(Arc::clone(&captured)));
        let logger = TracingLogger;

        tracing::subscriber::with_default(subscriber, || {
            logger.info(
                "test_event",
                log_fields([("key", LogFieldValue::from("value"))]),
            );
        });

        assert_eq!(
            captured.lock().expect("lock captured targets").as_slice(),
            [OTEL_APP_LOG_TARGET]
        );
    }

    #[test]
    fn otel_span_filter_enables_rig_spans_and_tracing_logger_events_at_info() {
        let filter = otel_span_filter();

        assert!(filter.would_enable("rig::completions", &tracing::Level::INFO));
        assert!(filter.would_enable("rig::agent::completion", &tracing::Level::INFO));
        assert!(filter.would_enable(OTEL_APP_LOG_TARGET, &tracing::Level::INFO));
    }

    #[test]
    fn otel_span_filter_rejects_unrelated_targets_and_below_info_level() {
        let filter = otel_span_filter();

        assert!(!filter.would_enable("reili_adapters::inbound::slack", &tracing::Level::INFO));
        assert!(!filter.would_enable(OTEL_SPAN_TARGET, &tracing::Level::DEBUG));
        assert!(!filter.would_enable(OTEL_APP_LOG_TARGET, &tracing::Level::DEBUG));
    }
}
