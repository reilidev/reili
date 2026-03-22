use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use reili_core::task::LlmUsageSnapshot;
use rig::completion::Usage;

#[derive(Clone, Default)]
pub struct LlmUsageCollector {
    requests: Arc<AtomicU32>,
    input_tokens: Arc<AtomicU64>,
    output_tokens: Arc<AtomicU64>,
    total_tokens: Arc<AtomicU64>,
}

impl LlmUsageCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_usage(&self, usage: &Usage) {
        self.input_tokens
            .fetch_add(usage.input_tokens, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(usage.output_tokens, Ordering::Relaxed);
        self.total_tokens
            .fetch_add(total_tokens_from_usage(usage), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> LlmUsageSnapshot {
        LlmUsageSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            total_tokens: self.total_tokens.load(Ordering::Relaxed),
        }
    }
}

fn total_tokens_from_usage(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input_tokens + usage.output_tokens
    }
}

#[cfg(test)]
mod tests {
    use reili_core::task::LlmUsageSnapshot;
    use rig::completion::Usage;

    use super::LlmUsageCollector;

    #[test]
    fn aggregates_requests_and_usage() {
        let collector = LlmUsageCollector::new();

        collector.record_request();
        collector.record_request();
        collector.record_usage(&Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cached_input_tokens: 0,
        });
        collector.record_usage(&Usage {
            input_tokens: 3,
            output_tokens: 7,
            total_tokens: 0,
            cached_input_tokens: 0,
        });

        assert_eq!(
            collector.snapshot(),
            LlmUsageSnapshot {
                requests: 2,
                input_tokens: 13,
                output_tokens: 27,
                total_tokens: 40,
            }
        );
    }
}
