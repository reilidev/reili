use rig::completion::Usage;
use reili_shared::types::LlmUsageSnapshot;

pub struct MapRigUsageToSnapshotInput {
    pub usage: Option<Usage>,
    pub requests: u32,
}

#[must_use]
pub fn map_rig_usage_to_llm_usage_snapshot(input: MapRigUsageToSnapshotInput) -> LlmUsageSnapshot {
    let usage = input.usage.unwrap_or_default();
    let total_tokens = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input_tokens + usage.output_tokens
    };

    LlmUsageSnapshot {
        requests: input.requests,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens,
    }
}

#[cfg(test)]
mod tests {
    use rig::completion::Usage;
    use reili_shared::types::LlmUsageSnapshot;

    use super::{MapRigUsageToSnapshotInput, map_rig_usage_to_llm_usage_snapshot};

    #[test]
    fn maps_usage_to_snapshot() {
        let snapshot = map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                cached_input_tokens: 0,
            }),
            requests: 2,
        });

        assert_eq!(
            snapshot,
            LlmUsageSnapshot {
                requests: 2,
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
            }
        );
    }

    #[test]
    fn falls_back_to_input_plus_output_when_total_is_missing() {
        let snapshot = map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
            usage: Some(Usage {
                input_tokens: 3,
                output_tokens: 7,
                total_tokens: 0,
                cached_input_tokens: 0,
            }),
            requests: 1,
        });

        assert_eq!(
            snapshot,
            LlmUsageSnapshot {
                requests: 1,
                input_tokens: 3,
                output_tokens: 7,
                total_tokens: 10,
            }
        );
    }

    #[test]
    fn returns_empty_snapshot_when_usage_is_missing() {
        let snapshot = map_rig_usage_to_llm_usage_snapshot(MapRigUsageToSnapshotInput {
            usage: None,
            requests: 0,
        });

        assert_eq!(
            snapshot,
            LlmUsageSnapshot {
                requests: 0,
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
            }
        );
    }
}
