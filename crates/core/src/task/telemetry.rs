use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmUsageSnapshot {
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::LlmUsageSnapshot;

    fn snapshot(value: u64) -> LlmUsageSnapshot {
        LlmUsageSnapshot {
            requests: 1,
            input_tokens: value,
            output_tokens: value,
            total_tokens: value * 2,
        }
    }

    #[test]
    fn serializes_and_deserializes_llm_usage_snapshot() {
        let value = snapshot(10);

        let json = serde_json::to_string(&value).expect("serialize snapshot");
        let restored: LlmUsageSnapshot = serde_json::from_str(&json).expect("deserialize snapshot");

        assert_eq!(restored, value);
    }
}
