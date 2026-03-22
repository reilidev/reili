use reili_core::task::LlmUsageSnapshot;

const EMPTY_LLM_USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
    requests: 0,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
};

pub fn create_empty_llm_usage_snapshot() -> LlmUsageSnapshot {
    EMPTY_LLM_USAGE_SNAPSHOT.clone()
}

#[cfg(test)]
mod tests {
    use super::create_empty_llm_usage_snapshot;

    #[test]
    fn returns_empty_usage_snapshot() {
        let value = create_empty_llm_usage_snapshot();

        assert_eq!(value.requests, 0);
        assert_eq!(value.input_tokens, 0);
        assert_eq!(value.output_tokens, 0);
        assert_eq!(value.total_tokens, 0);
    }
}
