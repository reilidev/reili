use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry, fmt};

pub fn init_json_logger() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    let subscriber = build_json_subscriber(std::io::stderr);
    tracing::subscriber::set_global_default(subscriber)
}

pub fn build_json_subscriber<W>(make_writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    Registry::default().with(default_env_filter()).with(
        fmt::layer()
            .json()
            .with_target(false)
            .with_current_span(false)
            .with_span_list(false)
            .with_ansi(false)
            .flatten_event(true)
            .with_writer(make_writer),
    )
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
}
