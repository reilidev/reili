use reili_shared::types::{
    BuildInvestigationLlmTelemetryInput, InvestigationLlmTelemetry, LlmUsageSnapshot,
};

const EMPTY_LLM_USAGE_SNAPSHOT: LlmUsageSnapshot = LlmUsageSnapshot {
    requests: 0,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
};

#[must_use]
pub fn create_empty_llm_usage_snapshot() -> LlmUsageSnapshot {
    EMPTY_LLM_USAGE_SNAPSHOT.clone()
}

#[must_use]
pub fn build_investigation_llm_telemetry(
    input: BuildInvestigationLlmTelemetryInput,
) -> InvestigationLlmTelemetry {
    InvestigationLlmTelemetry {
        coordinator: input.usage.clone(),
        total: input.usage,
    }
}

#[cfg(test)]
mod tests {
    use reili_shared::types::{BuildInvestigationLlmTelemetryInput, LlmUsageSnapshot};

    use super::{build_investigation_llm_telemetry, create_empty_llm_usage_snapshot};

    #[test]
    fn returns_empty_usage_snapshot() {
        let value = create_empty_llm_usage_snapshot();

        assert_eq!(value.requests, 0);
        assert_eq!(value.input_tokens, 0);
        assert_eq!(value.output_tokens, 0);
        assert_eq!(value.total_tokens, 0);
    }

    #[test]
    fn builds_telemetry_from_usage() {
        let telemetry = build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
            usage: snapshot(1, 10, 20, 30),
        });

        assert_eq!(telemetry.coordinator, snapshot(1, 10, 20, 30));
        assert_eq!(telemetry.total, snapshot(1, 10, 20, 30));
    }

    fn snapshot(
        requests: u32,
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
    ) -> LlmUsageSnapshot {
        LlmUsageSnapshot {
            requests,
            input_tokens,
            output_tokens,
            total_tokens,
        }
    }
}
