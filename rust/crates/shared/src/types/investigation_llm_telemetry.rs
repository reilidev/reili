use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsageSnapshot {
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvestigationLlmTelemetry {
    pub coordinator: LlmUsageSnapshot,
    pub synthesizer: LlmUsageSnapshot,
    pub total: LlmUsageSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInvestigationLlmTelemetryInput {
    pub coordinator_usage: LlmUsageSnapshot,
    pub synthesizer_usage: LlmUsageSnapshot,
}

#[cfg(test)]
mod tests {
    use super::{BuildInvestigationLlmTelemetryInput, InvestigationLlmTelemetry, LlmUsageSnapshot};

    fn snapshot(value: u64) -> LlmUsageSnapshot {
        LlmUsageSnapshot {
            requests: 1,
            input_tokens: value,
            output_tokens: value,
            total_tokens: value * 2,
        }
    }

    #[test]
    fn serializes_and_deserializes_llm_types() {
        let telemetry = InvestigationLlmTelemetry {
            coordinator: snapshot(10),
            synthesizer: snapshot(20),
            total: snapshot(30),
        };
        let build_input = BuildInvestigationLlmTelemetryInput {
            coordinator_usage: snapshot(10),
            synthesizer_usage: snapshot(20),
        };

        let telemetry_json = serde_json::to_string(&telemetry).expect("serialize telemetry");
        let input_json = serde_json::to_string(&build_input).expect("serialize build input");

        let restored_telemetry: InvestigationLlmTelemetry =
            serde_json::from_str(&telemetry_json).expect("deserialize telemetry");
        let restored_input: BuildInvestigationLlmTelemetryInput =
            serde_json::from_str(&input_json).expect("deserialize build input");

        assert_eq!(restored_telemetry, telemetry);
        assert_eq!(restored_input, build_input);
    }
}
