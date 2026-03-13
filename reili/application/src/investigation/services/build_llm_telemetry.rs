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
        coordinator: input.coordinator_usage.clone(),
        synthesizer: input.synthesizer_usage.clone(),
        total: add_llm_usage_snapshots(&input.coordinator_usage, &input.synthesizer_usage),
    }
}

fn add_llm_usage_snapshots(left: &LlmUsageSnapshot, right: &LlmUsageSnapshot) -> LlmUsageSnapshot {
    LlmUsageSnapshot {
        requests: left.requests + right.requests,
        input_tokens: left.input_tokens + right.input_tokens,
        output_tokens: left.output_tokens + right.output_tokens,
        total_tokens: left.total_tokens + right.total_tokens,
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
    fn builds_total_usage_by_adding_both_snapshots() {
        let telemetry = build_investigation_llm_telemetry(BuildInvestigationLlmTelemetryInput {
            coordinator_usage: snapshot(1, 10, 20, 30),
            synthesizer_usage: snapshot(2, 40, 50, 90),
        });

        assert_eq!(telemetry.coordinator, snapshot(1, 10, 20, 30));
        assert_eq!(telemetry.synthesizer, snapshot(2, 40, 50, 90));
        assert_eq!(telemetry.total, snapshot(3, 50, 70, 120));
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
