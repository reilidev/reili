mod log_entry;
mod log_field_value;
mod log_fields;
mod log_level;
#[path = "logger.rs"]
mod port;

pub use log_entry::LogEntry;
pub use log_field_value::LogFieldValue;
pub use log_fields::{LogFields, log_fields};
pub use log_level::LogLevel;
pub use port::Logger;

#[cfg(any(test, feature = "test-support"))]
pub use port::MockLogger;

#[cfg(test)]
mod tests;
